use aevo_rust_sdk::{
    aevo::{self, AevoClient, ClientCredentials as AevoClientCredentials}, 
    env::ENV, 
    rest::{OrderData, RestResponse}, 
    ws_structs::{BookTicker, Fill, WsResponse, WsResponseData}
};
use ethers::signers::{LocalWallet, Signer}; 
use dotenv::dotenv;
use env_logger;
use futures::{SinkExt, StreamExt};
use log::{error, info};
use reqwest;
use std::{self, char::MAX, sync::Arc};
use tokio::sync::{broadcast::error, Mutex};
use tokio::{join, sync::mpsc};
use eyre::{eyre, Result};
use serde_derive::{Deserialize, Serialize};
use hyperliquid_rust_sdk::{
    ClientLimit, ClientOrder, ClientOrderRequest, 
    ExchangeClient as HLExchangeClient, ExchangeDataStatus, 
    ExchangeDataStatuses, ExchangeResponse, ExchangeResponseStatus, 
    InfoClient as HLInfoClient, Message as HlMessage, Subscription, UserData
};

use crate::HLCredentials; 

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged, rename_all = "snake_case")]
pub enum AevoOrderStatus {
    Filled, 
    Partial, 
    Opened, 
    Cancelled, 
    Expired, 
    Rejected, 
    StopOrder
}

#[derive(Serialize, Deserialize, Debug)]
pub struct HLState {
    pub bid_px : f64, 
    pub ask_px : f64, 
    pub buy_open_sz : f64, 
    pub sell_open_sz : f64
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AevoState {
    pub bid_px : f64, 
    pub ask_px : f64, 
    pub buy_open_sz : f64, 
    pub sell_open_sz : f64,
    pub bid_resting_order : RestingOrder, 
    pub ask_resting_order : RestingOrder, 
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RestingOrder {
    pub px : f64, 
    pub sz : f64, 
    pub order_id : String, 
    pub status : AevoOrderStatus, 
    pub filled : f64,
}

pub enum MMState {
    AevoBidFillWaiting,
    HLSellCompleted, 
    AevoAskFillWaiting, 
    HLBuyCompleted
}

pub struct XMM {
    pub aevo_state : AevoState,
    pub hl_state : HLState, 
    pub aevo_client : Arc<AevoClient>, 
    pub hl_info_client : HLInfoClient, 
    pub hl_exchange_client : HLExchangeClient, 
    pub asset : String,
    pub max_size : f64,
    pub state : MMState
}

impl XMM {
    pub async fn new(aevo_credentials: AevoClientCredentials, hl_credentials: HLCredentials, asset: String, max_size: f64) -> XMM {
        let aevo_client = AevoClient::new(Some(aevo_credentials), ENV::MAINNET).await.unwrap();
        let hl_info_client = HLInfoClient::new(None, None).await.unwrap(); 
        let hl_signer: LocalWallet = hl_credentials.api_key.parse().unwrap();
        let hl_exchange_client = HLExchangeClient::new(None, hl_signer, None, None, None).await.unwrap();

        XMM {
            aevo_state : AevoState{ 
                bid_px: -1.0, 
                ask_px: -1.0, 
                buy_open_sz: 0.0, 
                sell_open_sz: 0.0, 
                bid_resting_order: RestingOrder { px: -1.0, sz: 0.0, order_id: "".to_string(), status: AevoOrderStatus::Cancelled, filled : 0.0}, 
                ask_resting_order: RestingOrder { px: -1.0, sz: 0.0, order_id: "".to_string(), status: AevoOrderStatus::Cancelled, filled : 0.0} 
            }, 
            hl_state : HLState{ 
                bid_px: -1.0, 
                ask_px: -1.0, 
                buy_open_sz: 0.0, 
                sell_open_sz: 0.0 
            }, 
            aevo_client : Arc::new(aevo_client), 
            hl_info_client : hl_info_client, 
            hl_exchange_client : hl_exchange_client, 
            asset : asset,
            max_size : max_size, 
            state : MMState::AevoBidFillWaiting
        }
    }

    pub async fn start(&mut self) {
        let (aevo_tx, mut aevo_rx) = mpsc::unbounded_channel::<WsResponse>();
        let (hl_tx, mut hl_rx) = mpsc::unbounded_channel::<HlMessage>(); 

        let aevo_client = self.aevo_client.clone(); 
        tokio::spawn(async move {
            let _ = aevo_client.read_messages(aevo_tx).await.map_err(|e| error!("Read messages error: {}", e));
        });

        self.aevo_client.subscribe_book_ticker(self.asset.clone(), "PERPETUAL".to_string()).await.unwrap();
        self.aevo_client.subscribe_fills().await.unwrap(); 

        self.hl_info_client.subscribe(Subscription::L2Book { coin: self.asset.clone() }, hl_tx.clone()).await.unwrap();
        self.hl_info_client.subscribe(Subscription::UserEvents { user: self.hl_exchange_client.wallet.address() }, hl_tx.clone()).await.unwrap();

        loop {
            tokio::select! {
                msg = aevo_rx.recv() => {
                    match msg {
                        Some(WsResponse::SubscribeResponse {
                            data: WsResponseData::BookTickerData { timestamp, tickers },
                            ..
                        }) => {
                            self.process_aevo_tickers(&tickers[0]).await;
                        },
                        Some(WsResponse::SubscribeResponse {
                            data: WsResponseData::FillsData { 
                                timestamp, 
                                fill
                            }, 
                            .. 
                        }) => {
                            self.process_aevo_fills(fill).await; 
                        },
                        _ => {}
                    }
                }, 
                msg = hl_rx.recv() => {
                    match msg {
                        Some(HlMessage::L2Book(l2_book)) => {
                            let l2_book = l2_book.data;
                            self.hl_state.ask_px = l2_book.levels[1][0].px.parse().unwrap(); 
                            self.hl_state.bid_px = l2_book.levels[0][0].px.parse().unwrap(); 
                        }, 
                        _ => {}
                    }
                }
            }
        }
    }

    async fn process_aevo_tickers(&mut self, ticker : &BookTicker) {
        let (bid_px, ask_px): (f64, f64) = (ticker.bid.price.parse().unwrap(), ticker.ask.price.parse().unwrap());

        // Calculate the adjusted bid price
        let adjusted_bid_px = bid_px;
        let resting_order = &self.aevo_state.bid_resting_order; 
        let new_resting_order: Result<RestingOrder> = match resting_order.status {
            AevoOrderStatus::Opened | AevoOrderStatus::Partial => {
                self.aevo_client
                    .rest_edit_order(
                        &resting_order.order_id, 
                        ticker.instrument_id.parse().unwrap(), 
                        true, 
                        adjusted_bid_px, 
                        self.max_size, 
                        None
                    )
                    .await
                    .map(|e| match e {
                        RestResponse::EditOrder(OrderData{ 
                            order_id, 
                            amount, 
                            price, 
                            filled, 
                            order_status,  
                            ..
                        }) => {
                            RestingOrder{
                                px : price.parse().unwrap(), 
                                sz : amount.parse().unwrap(), 
                                order_id : order_id, 
                                status : parse_order_status(&order_status).unwrap(),
                                filled : filled.parse().unwrap(),
                            }
                        }, 
                        _ => {
                            unreachable!()
                        }
                    })
            }, 
            AevoOrderStatus::Cancelled | AevoOrderStatus::Rejected | AevoOrderStatus::Expired => {
                self.aevo_client
                    .rest_create_order(
                    ticker.instrument_id.parse().unwrap(), 
                    true, 
                    adjusted_bid_px, 
                    self.max_size, 
                    None
                    )
                    .await
                    .map(|e| match e {
                        RestResponse::CreateOrder(OrderData{ 
                            order_id, 
                            amount, 
                            price, 
                            filled, 
                            order_status,  
                            ..
                        }) => {
                            RestingOrder{
                                px : price.parse().unwrap(), 
                                sz : amount.parse().unwrap(), 
                                order_id : order_id, 
                                status : parse_order_status(&order_status).unwrap(),
                                filled : filled.parse().unwrap()
                            }
                        }, 
                        _ => {
                            unreachable!()
                        }
                    })
            }, 
            AevoOrderStatus::Filled => {
                Err(eyre!("Order Filled Already"))
            },
            _ => unreachable!()
        };

        match new_resting_order {
            Ok(resting_order) => {
                self.aevo_state.bid_resting_order = resting_order;
            }, 
            Err(e) => error!("{}", e)
        }
    }

    async fn process_aevo_fills(&mut self, fill : Fill) {
        let aevo_state = &mut self.aevo_state; 
        let aevo_is_buy = fill.side == "buy";  
        let order = if aevo_is_buy {&mut aevo_state.bid_resting_order} else {&mut aevo_state.ask_resting_order}; 
        if fill.order_id != order.order_id {return}

        let order_status = parse_order_status(&fill.order_status).unwrap();
        order.filled = order.filled + fill.filled.parse::<f64>().unwrap(); 
        order.status = order_status.clone();
        if aevo_is_buy {
            aevo_state.buy_open_sz = aevo_state.buy_open_sz + fill.filled.parse::<f64>().unwrap();
        } else {
            aevo_state.sell_open_sz = aevo_state.sell_open_sz + fill.filled.parse::<f64>().unwrap();
        }

        match order_status {
            AevoOrderStatus::Filled | AevoOrderStatus::Partial => {
                let hl_is_buy = !aevo_is_buy; 
                let hl_state = &mut self.hl_state;
                let hl_order_amount = if hl_is_buy {aevo_state.buy_open_sz - hl_state.sell_open_sz} else {aevo_state.sell_open_sz - hl_state.buy_open_sz}; 
                if hl_order_amount > 0.0 {
                    let response = self.hl_exchange_client.order(
                        ClientOrderRequest{
                            asset : self.asset.clone(), 
                            is_buy : hl_is_buy, 
                            reduce_only : false, 
                            limit_px: if hl_is_buy {hl_state.ask_px * 1.05} else {hl_state.bid_px * 0.95},  // 5 % max slippage
                            sz : hl_order_amount, 
                            cloid : None, 
                            order_type : ClientOrder::Limit(
                                ClientLimit {
                                    tif: "Ioc".to_string(),  // market order is agressive limit order with IOC TIF
                                }
                            )
                        }, 
                        None
                    ).await; 

                    match response {
                        Ok(order) => match order {
                            ExchangeResponseStatus::Ok(order) => {
                                if let Some(order) = order.data {
                                    if !order.statuses.is_empty() {
                                        match &order.statuses[0] {
                                            ExchangeDataStatus::Filled(order) => {
                                                if hl_is_buy {
                                                    hl_state.buy_open_sz = hl_state.buy_open_sz + order.total_sz.parse::<f64>().unwrap();
                                                    info!("Market Buy Order Filled with size : {} and avg price : {}", order.total_sz, order.avg_px);
                                                    panic!(); 
                                                } else {
                                                    hl_state.sell_open_sz = hl_state.sell_open_sz + order.total_sz.parse::<f64>().unwrap();
                                                    info!("Market Sell Order Filled with size : {} and avg price : {}", order.total_sz, order.avg_px);
                                                    panic!();
                                                } 
                                            },
                                            ExchangeDataStatus::Error(e) => {
                                                error!("Error with placing order: {e}")
                                            },
                                            _ => unreachable!(),
                                        }
                                    } else {
                                        error!("Exchange data statuses is empty when placing order: {order:?}")
                                    }
                                } else {
                                    error!("Exchange response data is empty when placing order: {order:?}")
                                }
                            }
                            ExchangeResponseStatus::Err(e) => {
                                error!("Error with placing order: {e}")
                            }
                        },
                        Err(e) => error!("Error with placing order: {e}"),
                    }
                }
            }, 
            AevoOrderStatus::Cancelled | AevoOrderStatus::Expired | AevoOrderStatus::Rejected => {
                unreachable!()
            }
            _ => unreachable!()
        }
    }
}

fn parse_order_status (status: &str) -> Result<AevoOrderStatus> {
    match status {
        "filled" => Ok(AevoOrderStatus::Filled),
        "partial" => Ok(AevoOrderStatus::Partial),
        "opened" => Ok(AevoOrderStatus::Opened),
        "cancelled" => Ok(AevoOrderStatus::Cancelled),
        "expired" => Ok(AevoOrderStatus::Expired),
        "rejected" => Ok(AevoOrderStatus::Rejected),
        "stop_order" => Ok(AevoOrderStatus::StopOrder),
        _ => Err(eyre!("Order Status parsing error")),
    }
}