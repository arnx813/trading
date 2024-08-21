use aevo_rust_sdk::{
    aevo::{self, ClientCredentials}, env::ENV, rest::{OrderData, RestResponse}, signature::Order, ws_structs::{Fill, WsResponse, WsResponseData}
};
use dotenv::dotenv;
use env_logger;
use futures::{SinkExt, StreamExt};
use log::{error, info};
use reqwest;
use std::{self, sync::Arc};
use tokio::sync::{broadcast::error, Mutex};
use tokio::{join, sync::mpsc};
use tokio_tungstenite::tungstenite::Message;
use eyre::{eyre, Result};
use serde_derive::{Deserialize, Serialize};
use tokio::time; 
use std::time::Duration; 

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged, rename_all = "snake_case")]
pub enum OrderStatus {
    Filled, 
    Partial, 
    Opened, 
    Cancelled, 
    Expired, 
    Rejected, 
    StopOrder
}

pub struct MMState {
    pub bid_resting_order : Option<RestingOrder>, 
    pub ask_resting_order : Option<RestingOrder>,
}

pub struct RestingOrder {
    pub px : f64, 
    pub sz : f64, 
    pub order_id : String, 
    pub status : OrderStatus, 
    pub filled : f64, 
    pub is_buy : bool
}

#[tokio::main]
pub async fn main() {
    env_logger::init();
    dotenv().ok(); 

    let (tx, mut rx) = mpsc::unbounded_channel::<WsResponse>();

    // Aevo Initialization
    let credentials = ClientCredentials {
        signing_key : std::env::var("SIGNING_KEY").unwrap(),
        wallet_address : std::env::var("WALLET_ADDRESS").unwrap(),
        wallet_private_key: None,
        api_key : std::env::var("API_KEY").unwrap(),
        api_secret : std::env::var("API_SECRET").unwrap(),
    };
    let client = Arc::new(aevo::AevoClient::new(Some(credentials), ENV::MAINNET).await.unwrap()); 

    // Spawn Aevo Websockets Message reading task
    let client_clone = client.clone();
    let msg_read_handle = tokio::spawn(async move {
        let _ = client_clone.read_messages(tx).await.map_err(|e| error!("Read messages error: {}", e));
    });

    // Relevant Subscriptions
    client.subscribe_book_ticker("POPCAT".to_string(), "PERPETUAL".to_string()).await.unwrap();
    client.subscribe_fills().await.unwrap(); 

    // MM State Tracking
    let mm_state = Arc::new(Mutex::new(MMState{
        bid_resting_order : None, 
        ask_resting_order : None,
    }));

    let mm_state_clone = mm_state.clone(); 
    let client_clone = client.clone();
    let msg_process_handle = tokio::spawn(async move {
        loop {
            let msg = rx.recv().await;
            match msg {
                Some(WsResponse::SubscribeResponse {
                    data: WsResponseData::BookTickerData { timestamp, tickers },
                    ..
                }) => {
                    let ticker = &tickers[0];
                    let (bid_px, ask_px): (f64, f64) = (
                        ticker.bid.price.parse().unwrap(),
                        ticker.ask.price.parse().unwrap(),
                    );

                    let spread = (ask_px - bid_px) * 2.0 / (ask_px + bid_px);

                    info!(
                        "The lowest ask: {}; The highest bid: {}; The spread: {}",
                        ask_px, bid_px, spread
                    );

                    // Calculate the adjusted bid price
                    let adjusted_bid_px = (0.99 * bid_px * 10_000.0).round() / 10_000.0;
                    {
                        let mut state_guard = mm_state_clone.lock().await; 
                        let new_resting_order: Result<RestingOrder> =  match &state_guard.bid_resting_order {
                            None => {
                                submit_order(
                                    &client_clone, 
                                    ticker.instrument_id.parse().unwrap(), 
                                    true, 
                                    adjusted_bid_px, 
                                    50.0
                                ).await
                            }, 
                            Some(RestingOrder {
                                order_id, 
                                px, 
                                sz, 
                                status, 
                                filled, 
                                is_buy
                            })=> {
                                match status {
                                    OrderStatus::Opened => {
                                        edit_order(
                                            &client, 
                                            order_id, 
                                            ticker.instrument_id.parse().unwrap(), 
                                            true, 
                                            adjusted_bid_px, 
                                            50.0
                                        ).await
                                    }, 
                                    OrderStatus::Filled => {
                                        Err(eyre!("Order Filled Already"))
                                    }, 
                                    OrderStatus::Partial => {
                                        edit_order(
                                            &client, 
                                            order_id, 
                                            ticker.instrument_id.parse().unwrap(), 
                                            true, 
                                            adjusted_bid_px, 
                                            100.0
                                        ).await
                                    },
                                    _ => {  // Order was Rejected, Expired or Cancelled
                                        submit_order(
                                            &client_clone, 
                                            ticker.instrument_id.parse().unwrap(), 
                                            true, 
                                            adjusted_bid_px, 
                                            50.0
                                        ).await
                                    },
                                }
                            }
                        }; 

                        match new_resting_order {
                            Ok(resting_order) => {
                                state_guard.bid_resting_order = Some(resting_order);
                            }, 
                            Err(e) => error!("{}", e)
                        }
                    }

                },
                Some(WsResponse::SubscribeResponse {
                    data: WsResponseData::FillsData { 
                        timestamp, 
                        fill: Fill { 
                            trade_id, 
                            order_id, 
                            instrument_id,
                            instrument_name, 
                            instrument_type, 
                            price, 
                            side, 
                            fees, 
                            filled, 
                            order_status, 
                            liquidity, 
                            created_timestamp, 
                            system_type 
                        } 
                    }, 
                    .. 
                }) => {
                    let is_buy = side == "buy"; 
                    {   
                        let mut state_guard = mm_state_clone.lock().await; 
                        match is_buy {
                            true => {
                                if let Some(order) = &mut state_guard.bid_resting_order {
                                    if order_id == order.order_id {
                                        order.filled = filled.parse().unwrap(); 
                                        order.status = serde_json::from_str(&order_status).unwrap(); 
                                    }
                                }
                            }, 
                            false => {
                                if let Some(order) = &mut state_guard.ask_resting_order {
                                    if order_id == order.order_id {
                                        order.filled = filled.parse().unwrap(); 
                                        order.status = serde_json::from_str(&order_status).unwrap(); 
                                    }
                                }
                            }
                        }
                    }
                },
                
                _ => {}

            }

            time::sleep(Duration::from_secs(1)).await; 
        }
    });

    join!(msg_read_handle, msg_process_handle);
}


async fn submit_order(
    client: &Arc<aevo::AevoClient>,
    instrument_id: u64, 
    is_buy: bool, 
    limit_price: f64, 
    quantity: f64
) -> Result<RestingOrder> {

    let response = client
        .rest_create_order(
            instrument_id, 
            is_buy, 
            limit_price, 
            quantity, 
            None
        )
        .await?; 

    match response {
        RestResponse::CreateOrder( OrderData { 
            order_id, 
            side,
            amount, 
            price, 
            avg_price, 
            filled, 
            order_status,  
            ..
        }) => {
            info!("Order created with ID: {}, order status {}", order_id, order_status);
            Ok(RestingOrder{
                px : price.parse().unwrap(), 
                sz : amount.parse().unwrap(), 
                order_id : order_id, 
                status : parse_order_status(&order_status).unwrap(),
                filled : filled.parse().unwrap(), 
                is_buy : is_buy
            })

        }, 
        _ => {
            Err(eyre!("Unexpected Create Rest Response : {:?}", response))
        }
    }
}

async fn edit_order(
    client: &Arc<aevo::AevoClient>,
    order_id : &String,
    instrument_id: u64, 
    is_buy: bool, 
    limit_price: f64, 
    quantity: f64
) -> Result<RestingOrder> {

    let response = client
        .rest_edit_order(
            order_id, 
            instrument_id, 
            is_buy, 
            limit_price, 
            quantity, 
            None
        )
        .await?; 
    
    match response {
        RestResponse::EditOrder(OrderData{ 
            order_id, 
            side,
            amount, 
            price, 
            avg_price, 
            filled, 
            order_status,  
            ..
        }) => {
            Ok(RestingOrder{
                px : price.parse().unwrap(), 
                sz : amount.parse().unwrap(), 
                order_id : order_id, 
                status : parse_order_status(&order_status).unwrap(),
                filled : filled.parse().unwrap(), 
                is_buy : is_buy
            })
        }, 
        _ => {
            Err(eyre!("Unexpected Edit Rest Response : {:?}", response))
        }
    }
}

fn parse_order_status (status: &str) -> Result<OrderStatus> {
    match status.to_lowercase().as_str() {
        "filled" => Ok(OrderStatus::Filled),
        "partial" => Ok(OrderStatus::Partial),
        "opened" => Ok(OrderStatus::Opened),
        "cancelled" => Ok(OrderStatus::Cancelled),
        "expired" => Ok(OrderStatus::Expired),
        "rejected" => Ok(OrderStatus::Rejected),
        "stop_order" => Ok(OrderStatus::StopOrder),
        _ => Err(eyre!("Order Status parsing error")),
    }
}   