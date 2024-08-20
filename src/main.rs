use aevo_rust_sdk::{
    aevo::{self, ClientCredentials}, env::ENV, signature::Order, ws_structs::{Fill, WsResponse, WsResponseData}
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

pub struct MMState {
    pub bid_resting_order : Option<RestingOrder>, 
    pub ask_resting_order : Option<RestingOrder>,
}

pub struct RestingOrder {
    pub px : f64, 
    pub sz : f64, 
    pub order_id : String, 
    pub verified : bool, 
    pub filled : bool
}

#[tokio::main]
pub async fn main() {
    env_logger::init();
    dotenv().ok(); 

    let (tx, mut rx) = mpsc::unbounded_channel::<WsResponse>();

    let signing_key = std::env::var("SIGNING_KEY").unwrap();
    let wallet_address = std::env::var("WALLET_ADDRESS").unwrap();
    let api_key = std::env::var("API_KEY").unwrap();
    let api_secret = std::env::var("API_SECRET").unwrap();

    let credentials = ClientCredentials {
        signing_key,
        wallet_address,
        wallet_private_key: None,
        api_key,
        api_secret,
    };

    let client = Arc::new(aevo::AevoClient::new(Some(credentials), ENV::MAINNET)
        .await
        .unwrap()); 

    let client_clone = client.clone();

    let msg_read_handle = tokio::spawn(async move {
        let _ = client_clone
            .read_messages(tx)
            .await
            .map_err(|e| error!("Read messages error: {}", e));
    });

    client.subscribe_book_ticker("POPCAT".to_string(), "PERPETUAL".to_string())
        .await
        .unwrap();

    let mm_state = Arc::new(Mutex::new(MMState{
        bid_resting_order : None, 
        ask_resting_order : None,
    }));

    let mm_state_clone = mm_state.clone(); 

    let client_clone = client.clone();

    let msg_process_handle = tokio::spawn(async move {
        loop {
            // info!("{}", edit_order_id);
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
                    info!("editing bid to {}", adjusted_bid_px);

                    {
                        let mut state_guard = mm_state_clone.lock().await; 
                        let new_resting_order: Result<RestingOrder> =  match &state_guard.bid_resting_order {
                            None => {
                                submit_order(&client_clone).await
                            }, 
                            Some(RestingOrder {
                                order_id, 
                                px, 
                                sz, 
                                verified, 
                                filled
                            })=> {
                                if !filled {
                                    edit_order(
                                        &client_clone, 
                                        order_id.to_string(), 
                                        adjusted_bid_px
                                    ).await
                                } else {
                                    Err(eyre!("Order Filled Already"))
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
                Some(WsResponse::PublishResponse {
                    data: WsResponseData::CreateEditOrderData {
                        order_id,
                        account,
                        instrument_id,
                        instrument_name,
                        instrument_type,
                        expiry,
                        strike,
                        option_type,
                        order_type,
                        order_status,
                        side,
                        amount,
                        price,
                        filled,
                        initial_margin,
                        avg_price,
                        created_timestamp,
                        timestamp,
                        system_type,
                    },
                    ..
                }) => {
                    {
                        let mut mm_state_guard = mm_state_clone.lock().await; 
                        if let Some(order) = &mut mm_state_guard.bid_resting_order {
                            if order.order_id == order_id {
                                order.verified = true; 
                                info!("The order {} is verified", order_id)
                            }
                        }
                    }
                },
                
                _ => {}

            }

            
        }
    });

    // join!(msg_read_handle, msg_process_handle);
}


async fn submit_order(
    client: &Arc<aevo::AevoClient>,
) -> Result<RestingOrder> {
    // Log test message

    let instrument_id = 36426; // Replace with actual instrument ID
    let is_buy = true; // Example: true for buy order
    let limit_price: f64 = 0.2; // Ensure this is f64
    let quantity: f64 = 100.0; // Ensure this is f64

    // You can use None for optional parameters you want to omit
    let post_only = None; // Omitting the post_only parameter
    let id = None; // Omitting the request ID parameter
    let mmp = None; // Omitting the mmp parameter

    let order_id = client
        .create_order(
            instrument_id,
            is_buy,
            limit_price,
            quantity,
            post_only,
            id,
            mmp,
        )
        .await?;

    info!("Order created with ID: {}", order_id);

    Ok(RestingOrder{
        px : limit_price, 
        sz : quantity, 
        order_id : order_id, 
        verified : false, 
        filled : false
    })
}

async fn edit_order(
    client: &Arc<aevo::AevoClient>,
    edit_order_id: String,
    new_bid_price: f64,
) -> Result<RestingOrder> {

    let instrument_id = 36426; // Replace with actual instrument ID
    let is_buy = true; // Example: true for buy order
    let limit_price: f64 = 0.2; // Ensure this is f64
    let quantity: f64 = 130.0; // Ensure this is f64

    // You can use None for optional parameters you want to omit
    let post_only = None; // Omitting the post_only parameter
    let id = None; // Omitting the request ID parameter
    let mmp = None; // Omitting the mmp parameter

    // Log all variables in one line
    info!(
    "Calling edit_order with params - order_id_string: {}, instrument_id: {}, is_buy: {}, new_bid_price: {}, quantity: {}, post_only: {:?}, id: {:?}, mmp: {:?}",
    edit_order_id, instrument_id, is_buy, new_bid_price, quantity, post_only, id, mmp
    );

    let new_order_id = client
        .edit_order(
            edit_order_id,
            instrument_id,
            is_buy,
            new_bid_price,
            quantity,
            post_only,
            id,
            mmp,
        )
        .await?;

    info!("Updating Order ID to: {}", new_order_id);

    Ok(RestingOrder{
        px : limit_price, 
        sz : quantity, 
        order_id : new_order_id, 
        verified : false, 
        filled : false
    })
}