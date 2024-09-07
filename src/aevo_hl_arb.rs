use crate::HLCredentials;
use aevo_rust_sdk::{
    aevo::{self, AevoClient, ClientCredentials as AevoClientCredentials},
    env::ENV,
    rest::{OrderData, RestResponse},
    ws_structs::{WsResponse, WsResponseData},
};
use ethers::signers::{LocalWallet, Signer};
use futures::join;
use hyperliquid_rust_sdk::{
    ClientLimit, ClientOrder, ClientOrderRequest, Error as HLError,
    ExchangeClient as HLExchangeClient, ExchangeDataStatus, ExchangeResponseStatus,
    InfoClient as HLInfoClient, Message as HlMessage, Subscription,
};
use log::{error, info};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, Mutex},
    time,
};

pub struct XArb {
    aevo_state: ExchangeState,
    hl_state: ExchangeState,
    aevo_client: Arc<AevoClient>,
    hl_info_client: HLInfoClient,
    hl_exchange_client: Arc<HLExchangeClient>,
    asset: String,
    max_size: f64,
    instrument_id: u64,
    min_delta_bps: u16,
    is_processing_flag: bool,
}

#[derive(Debug, Clone)]
pub struct ExchangeState {
    pub bid_px: f64,
    pub ask_px: f64,
    pub buy_open_sz: f64,
    pub sell_open_sz: f64,
}

pub struct ArbOrder {
    pub aevo_is_buy: bool,
    pub aevo_px: f64,
    pub hl_px: f64,
    pub aevo_sz: f64,
    pub hl_sz: f64,
    pub instrument_id: u64,
    pub asset: String,
}

impl XArb {
    pub async fn new(
        aevo_credentials: AevoClientCredentials,
        hl_credentials: HLCredentials,
        asset: String,
        max_size: f64,
        instrument_id: u64,
        min_delta_bps: u16,
    ) -> XArb {
        let aevo_client = AevoClient::new(Some(aevo_credentials), ENV::MAINNET)
            .await
            .unwrap();
        let hl_info_client = HLInfoClient::new(None, None).await.unwrap();
        let hl_signer: LocalWallet = hl_credentials.api_key.parse().unwrap();
        let hl_exchange_client = HLExchangeClient::new(None, hl_signer, None, None, None)
            .await
            .unwrap();

        XArb {
            aevo_state: ExchangeState {
                bid_px: -1.0,
                ask_px: -1.0,
                buy_open_sz: 0.0,
                sell_open_sz: 0.0,
            },
            hl_state: ExchangeState {
                bid_px: -1.0,
                ask_px: -1.0,
                buy_open_sz: 0.0,
                sell_open_sz: 0.0,
            },
            aevo_client: Arc::new(aevo_client),
            hl_info_client,
            hl_exchange_client: Arc::new(hl_exchange_client),
            asset,
            max_size,
            instrument_id,
            min_delta_bps,
            is_processing_flag: false,
        }
    }

    pub async fn start(&mut self) {
        let (aevo_tx, mut aevo_rx) = mpsc::unbounded_channel::<WsResponse>();
        let (hl_tx, mut hl_rx) = mpsc::unbounded_channel::<HlMessage>();
        let (order_response_tx, mut order_response_rx) = mpsc::unbounded_channel::<(
            eyre::Result<RestResponse>,
            Result<ExchangeResponseStatus, HLError>,
        )>();

        let aevo_client = self.aevo_client.clone();
        tokio::spawn(async move {
            let _ = aevo_client
                .read_messages(aevo_tx)
                .await
                .map_err(|e| error!("Read messages error: {}", e));
        });

        let aevo_client = self.aevo_client.clone();
        tokio::spawn(async move {
            loop {
                let _ = aevo_client.ping().await;
                time::sleep(Duration::from_secs(60)).await; //ping every 60 secs to keep ws connection alive
            }
        });

        self.aevo_client
            .subscribe_book_ticker(self.asset.clone(), "PERPETUAL".to_string())
            .await
            .unwrap();
        self.hl_info_client
            .subscribe(
                Subscription::L2Book {
                    coin: self.asset.clone(),
                },
                hl_tx.clone(),
            )
            .await
            .unwrap();

        loop {
            tokio::select! {
                msg = aevo_rx.recv() => {
                    if let Some(WsResponse::SubscribeResponse {
                        data: WsResponseData::BookTickerData { tickers, .. },
                        ..
                    }) = msg {
                        let ticker = &tickers[0];
                        self.aevo_state.ask_px = ticker.ask.price.parse().unwrap();
                        self.aevo_state.bid_px = ticker.bid.price.parse().unwrap();
                        info!(
                            "Aevo task : AEVO STATE : {:?}, HL STATE : {:?}, IS LOCKED : {}",
                            self.aevo_state, self.hl_state, self.is_processing_flag
                        );

                        if self.hl_state.ask_px > 0.0 && self.hl_state.bid_px > 0.0 && !self.is_processing_flag {
                            let arb_order = self.prepare_order();
                            if let Some(order) = arb_order {
                                self.is_processing_flag = true;
                                let aevo_client = self.aevo_client.clone();
                                let hl_client = self.hl_exchange_client.clone();
                                let order_response_tx = order_response_tx.clone();
                                tokio::spawn(async move {
                                    order_response_tx.send(
                                        XArb::submit_orders(
                                            aevo_client,
                                            hl_client,
                                            order
                                        ).await
                                    ).unwrap()
                                });
                            }
                        }
                    }
                },
                msg = hl_rx.recv() => {
                    if let Some(HlMessage::L2Book(l2_book)) = msg {
                        let l2_book = l2_book.data;

                        self.hl_state.ask_px = l2_book.levels[1][0].px.parse().unwrap();
                        self.hl_state.bid_px = l2_book.levels[0][0].px.parse().unwrap();
                        info!(
                            "HL Task : AEVO STATE : {:?}, HL STATE : {:?}, IS LOCKED : {}",
                            self.aevo_state, self.hl_state, self.is_processing_flag
                        );

                        if self.aevo_state.ask_px > 0.0 && self.aevo_state.bid_px > 0.0 && !self.is_processing_flag {
                            let arb_order = self.prepare_order();
                            if let Some(order) = arb_order {
                                let aevo_client = self.aevo_client.clone();
                                let hl_client = self.hl_exchange_client.clone();
                                let order_response_tx = order_response_tx.clone();
                                self.is_processing_flag = true;
                                tokio::spawn(async move {
                                    order_response_tx.send(
                                        XArb::submit_orders(
                                            aevo_client,
                                            hl_client,
                                            order
                                        ).await
                                    ).unwrap()
                                });
                            }
                        }
                    }
                },
                responses = order_response_rx.recv() => {
                    if let Some((aevo_response, hl_response)) = responses {
                        let aevo_is_buy: bool;
                        match aevo_response {
                            Ok(RestResponse::CreateOrder(OrderData {
                                filled, avg_price, side, ..
                            })) => {
                                let amount_filled = filled.parse::<f64>().unwrap();
                                if side == "buy".to_string() {
                                    self.aevo_state.buy_open_sz += amount_filled;
                                    self.aevo_state.sell_open_sz -= amount_filled;
                                    aevo_is_buy = true;
                                } else if side == "sell".to_string() {
                                    self.aevo_state.buy_open_sz -= amount_filled;
                                    self.aevo_state.sell_open_sz += amount_filled;
                                    aevo_is_buy = false;
                                } else {
                                    unreachable!()
                                }
                                info!(
                                    "Aevo: Market {side} Order Filled with size : {} and avg price : {:?}",
                                    filled, avg_price
                                );
                            }
                            Err(e) => panic!("Error creating aevo order : {}", e),
                            _ => unreachable!(),
                        }

                        match hl_response {
                            Ok(order) => match order {
                                ExchangeResponseStatus::Ok(order) => {
                                    if let Some(order) = order.data {
                                        if !order.statuses.is_empty() {
                                            match &order.statuses[0] {
                                                ExchangeDataStatus::Filled(order) => {
                                                    let amount_filled = order.total_sz.parse::<f64>().unwrap();
                                                    if aevo_is_buy {
                                                        self.hl_state.sell_open_sz += amount_filled;
                                                        self.hl_state.buy_open_sz -= amount_filled;
                                                    } else {
                                                        self.hl_state.sell_open_sz -= amount_filled;
                                                        self.hl_state.buy_open_sz += amount_filled;
                                                    }
                                                    info!("Hyperliquid: Market Order Filled with size : {} and avg price : {}", order.total_sz, order.avg_px);
                                                }
                                                ExchangeDataStatus::Error(e) => {
                                                    panic!("Error with placing order: {e}")
                                                }
                                                _ => unreachable!(),
                                            }
                                        } else {
                                            panic!(
                                                "Exchange data statuses is empty when placing order: {order:?}"
                                            )
                                        }
                                    } else {
                                        panic!("Exchange response data is empty when placing order: {order:?}")
                                    }
                                }
                                ExchangeResponseStatus::Err(e) => {
                                    panic!("Error with placing order: {e}")
                                }
                            },
                            Err(e) => panic!("Error with placing order: {e}"),
                        }
                        self.is_processing_flag = false;
                    }
                }
            }
        }
    }

    async fn submit_orders(
        aevo_client: Arc<AevoClient>,
        hl_client: Arc<HLExchangeClient>,
        order: ArbOrder,
    ) -> (
        eyre::Result<RestResponse>,
        Result<ExchangeResponseStatus, HLError>,
    ) {
        let (aevo_limit_px, hl_limit_px) = {
            if order.aevo_is_buy {
                (
                    (order.aevo_px * 1.05 * 100.0).round() / 100.0,
                    (order.hl_px * 10.0 * 0.95).round() / 10.0,
                )
            } else {
                (
                    (order.aevo_px * 0.95 * 100.0).round() / 100.0,
                    (order.hl_px * 10.0 * 1.05).round() / 10.0,
                )
            }
        };

        let aevo_handle = aevo_client.rest_create_order(
            order.instrument_id,
            order.aevo_is_buy,
            aevo_limit_px, // 5% slippage
            order.aevo_sz,
            Some(false),
            Some("IOC".to_string()),
        );

        let hl_handle = hl_client.order(
            ClientOrderRequest {
                asset: order.asset,
                is_buy: !order.aevo_is_buy,
                reduce_only: false,
                limit_px: hl_limit_px, // 5% slippage
                sz: order.hl_sz,
                cloid: None,
                order_type: ClientOrder::Limit(ClientLimit {
                    tif: "Ioc".to_string(), // market order is agressive limit order with IOC TIF
                }),
            },
            None,
        );
        let (aevo_response, hl_response) = join!(aevo_handle, hl_handle);
        (aevo_response, hl_response)
    }

    fn prepare_order(&self) -> Option<ArbOrder> {
        let spread_1 =
            (self.hl_state.bid_px - self.aevo_state.ask_px) / self.aevo_state.ask_px * 10_000.0;
        let spread_2 =
            (self.aevo_state.bid_px - self.hl_state.ask_px) / self.hl_state.ask_px * 10_000.0;
        info!("Spread 1 : {:.4}, Spread 2 : {:.4}", spread_1, spread_2);

        if spread_1 > self.min_delta_bps as f64 && self.aevo_state.buy_open_sz < self.max_size {
            let order_amount = if self.aevo_state.buy_open_sz >= 0.0 {
                self.max_size - self.aevo_state.buy_open_sz
            } else {
                -self.aevo_state.buy_open_sz
            }; 
            Some(ArbOrder {
                aevo_is_buy: true,
                aevo_px: self.aevo_state.ask_px,
                hl_px: self.hl_state.bid_px,
                aevo_sz: order_amount,
                hl_sz: order_amount,
                instrument_id: self.instrument_id,
                asset: self.asset.clone(),
            })
        } else if spread_2 > self.min_delta_bps as f64
            && self.aevo_state.sell_open_sz < self.max_size
        {
            let order_amount = if self.aevo_state.sell_open_sz >= 0.0 {
                self.max_size - self.aevo_state.sell_open_sz
            } else {
                -self.aevo_state.sell_open_sz
            };
            //let order_amount = self.max_size - self.aevo_state.sell_open_sz;
            Some(ArbOrder {
                aevo_is_buy: false,
                aevo_px: self.aevo_state.bid_px,
                hl_px: self.hl_state.ask_px,
                aevo_sz: order_amount,
                hl_sz: order_amount,
                instrument_id: self.instrument_id,
                asset: self.asset.clone(),
            })
        } else {
            None
        }
    }
}
