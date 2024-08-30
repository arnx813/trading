use crate::HLCredentials;
use aevo_rust_sdk::{
    aevo::{self, AevoClient, ClientCredentials as AevoClientCredentials},
    env::ENV,
    rest::{OrderData, RestResponse},
    ws_structs::{BookTicker, Fill, WsResponse, WsResponseData},
};
use ethers::signers::{LocalWallet, Signer};
use futures::join;
use hyperliquid_rust_sdk::{
    ClientLimit, ClientOrder, ClientOrderRequest, ExchangeClient as HLExchangeClient,
    ExchangeDataStatus, ExchangeDataStatuses, ExchangeResponse, ExchangeResponseStatus,
    InfoClient as HLInfoClient, Message as HlMessage, Subscription, UserData,
};
use log::{error, info};
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc, time};

pub struct XArb {
    aevo_state: ExchangeState,
    hl_state: ExchangeState,
    aevo_client: Arc<AevoClient>,
    hl_info_client: HLInfoClient,
    hl_exchange_client: HLExchangeClient,
    asset: String,
    max_size: f64,
    instrument_id: u64,
    min_delta_bps: u16,
}

#[derive(Debug)]
pub struct ExchangeState {
    pub bid_px: f64,
    pub ask_px: f64,
    pub buy_open_sz: f64,
    pub sell_open_sz: f64,
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
            hl_exchange_client,
            asset,
            max_size,
            instrument_id,
            min_delta_bps,
        }
    }

    pub async fn start(&mut self) {
        let (aevo_tx, mut aevo_rx) = mpsc::unbounded_channel::<WsResponse>();
        let (hl_tx, mut hl_rx) = mpsc::unbounded_channel::<HlMessage>();

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

        let mut aevo_buffer: Vec<WsResponse> = Vec::new();
        let mut hl_buffer: Vec<HlMessage> = Vec::new();
        loop {
            println!(
                "Aevo State : {:?}, HL State : {:?}",
                self.aevo_state, self.hl_state
            );
            tokio::select! {
                msg_count = aevo_rx.recv_many(&mut aevo_buffer, 100) => {
                    if msg_count > 0 {
                        match aevo_buffer.pop() {
                            Some(WsResponse::SubscribeResponse {
                                data: WsResponseData::BookTickerData {tickers, ..},
                                ..
                            }) => {
                                let ticker = &tickers[0];
                                self.aevo_state.ask_px = ticker.ask.price.parse().unwrap();
                                self.aevo_state.bid_px = ticker.bid.price.parse().unwrap();
                                if self.hl_state.ask_px > 0.0 && self.hl_state.bid_px > 0.0 {
                                    self.submit_orders().await;
                                }
                            },
                            _ => {}
                        }
                        aevo_buffer.clear();
                    }
                },
                msg_count = hl_rx.recv_many(&mut hl_buffer, 100) => {
                    if msg_count > 0 {
                        match hl_buffer.pop() {
                            Some(HlMessage::L2Book(l2_book)) => {
                                let l2_book = l2_book.data;
                                self.hl_state.ask_px = l2_book.levels[1][0].px.parse().unwrap();
                                self.hl_state.bid_px = l2_book.levels[0][0].px.parse().unwrap();

                                if self.aevo_state.ask_px > 0.0 && self.aevo_state.bid_px > 0.0 {
                                    self.submit_orders().await;
                                }
                            },
                            _ => {}
                        }
                        hl_buffer.clear();
                    }
                },
            }
        }
    }

    async fn submit_orders(&mut self) {
        if ((self.hl_state.bid_px - self.aevo_state.ask_px) / self.aevo_state.ask_px * 10_000.0)
            > self.min_delta_bps as f64
            && self.aevo_state.buy_open_sz < self.max_size
        {
            let order_amount = self.max_size - self.aevo_state.buy_open_sz;
            let aevo_handle = self.aevo_client.rest_create_order(
                self.instrument_id,
                true,
                (self.aevo_state.ask_px * 1.05 * 10_000.0).round() / 10_000.0, // 5% slippage
                order_amount,
                Some(false),
                Some("IOC".to_string()),
            );
            let hl_handle = self.hl_exchange_client.order(
                ClientOrderRequest {
                    asset: self.asset.clone(),
                    is_buy: false,
                    reduce_only: false,
                    limit_px: (self.hl_state.bid_px * 100_000.0 * 0.95).round() / 100_000.0, // 5% slippage
                    sz: order_amount,
                    cloid: None,
                    order_type: ClientOrder::Limit(ClientLimit {
                        tif: "Ioc".to_string(), // market order is agressive limit order with IOC TIF
                    }),
                },
                None,
            );

            let (aevo_response, hl_response) = join!(aevo_handle, hl_handle);

            match aevo_response {
                Ok(RestResponse::CreateOrder(OrderData {
                    filled, avg_price, ..
                })) => {
                    let amount_filled = filled.parse::<f64>().unwrap();
                    self.aevo_state.buy_open_sz = self.aevo_state.buy_open_sz + amount_filled;
                    self.aevo_state.sell_open_sz = self.aevo_state.sell_open_sz - amount_filled;
                    info!(
                        "Aevo: Market Buy Order Filled with size : {} and avg price : {:?}",
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
                                        self.hl_state.sell_open_sz =
                                            self.hl_state.sell_open_sz + amount_filled;
                                        self.hl_state.buy_open_sz =
                                            self.hl_state.buy_open_sz - amount_filled;
                                        info!("Hyperliquid: Market Sell Order Filled with size : {} and avg price : {}", order.total_sz, order.avg_px);
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
        } else if ((self.aevo_state.bid_px - self.hl_state.ask_px) / self.hl_state.ask_px
            * 10_000.0)
            > self.min_delta_bps as f64
            && self.aevo_state.sell_open_sz < self.max_size
        {
            let order_amount = self.max_size - self.aevo_state.sell_open_sz;
            let aevo_handle = self.aevo_client.rest_create_order(
                self.instrument_id,
                false,
                (self.aevo_state.bid_px * 0.95 * 10_000.0).round() / 10_000.0, // 5% slippage
                order_amount,
                Some(false),
                Some("IOC".to_string()),
            );
            let hl_handle = self.hl_exchange_client.order(
                ClientOrderRequest {
                    asset: self.asset.clone(),
                    is_buy: true,
                    reduce_only: false,
                    limit_px: (self.hl_state.bid_px * 100_000.0 * 1.05).round() / 100_000.0, // 5% slippage
                    sz: order_amount,
                    cloid: None,
                    order_type: ClientOrder::Limit(ClientLimit {
                        tif: "Ioc".to_string(), // market order is agressive limit order with IOC TIF
                    }),
                },
                None,
            );

            let (aevo_response, hl_response) = join!(aevo_handle, hl_handle);

            match aevo_response {
                Ok(RestResponse::CreateOrder(OrderData {
                    filled, avg_price, ..
                })) => {
                    let amount_filled = filled.parse::<f64>().unwrap();
                    self.aevo_state.sell_open_sz = self.aevo_state.sell_open_sz + amount_filled;
                    self.aevo_state.buy_open_sz = self.aevo_state.buy_open_sz - amount_filled;
                    info!(
                        "Aevo: Market Sell Order Filled with size : {} and avg price : {:?}",
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
                                        self.hl_state.buy_open_sz =
                                            self.hl_state.buy_open_sz + amount_filled;
                                        self.hl_state.sell_open_sz =
                                            self.hl_state.sell_open_sz - amount_filled;
                                        info!("Hyperliquid: Market Buy Order Filled with size : {} and avg price : {}", order.total_sz, order.avg_px);
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
        }
    }
}
