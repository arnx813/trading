use aevo_rust_sdk::{
    aevo::{self, ClientCredentials},
    env::ENV,
    ws_structs::{Fill, WsResponse, WsResponseData},
};
use dotenv::dotenv;
use env_logger;
use futures::{SinkExt, StreamExt};
use log::{error, info};
use reqwest;
use std::{self, sync::Arc};
use tokio::sync::Mutex;
use tokio::{join, sync::mpsc};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
pub async fn main() {
    env_logger::init();
    dotenv().ok();

    let (tx, mut rx) = mpsc::unbounded_channel::<WsResponse>();
    let edit_order_id = Arc::new(Mutex::new(
        "0x5b75fc0355d4baa1df7edeb30771b38f6ab6bb72c370229eb2533bb0b150216c".to_string(),
    ));

    let edit_order_id_clone = Arc::clone(&edit_order_id);

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

    let client = Arc::new(
        aevo::AevoClient::new(Some(credentials), ENV::MAINNET)
            .await
            .unwrap(),
    );

    async fn submit_order(
        client: &Arc<aevo::AevoClient>,
    ) -> Result<(), Box<dyn std::error::Error>> {
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

        Ok(())
    }

    // if let Err(e) = submit_order(&client).await {
    //     error!("Error submitting order: {}", e);
    // }

    async fn edit_order(
        client: &Arc<aevo::AevoClient>,
        edit_order_id: Arc<Mutex<String>>,
        new_bid_price: f64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Lock the Mutex to access the value
        let order_id = edit_order_id.lock().await;

        // Clone the inner value so you can use it after releasing the lock
        let order_id_string = order_id.clone(); // Clone the string
        drop(order_id); // Release the lock early

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
        order_id_string, instrument_id, is_buy, new_bid_price, quantity, post_only, id, mmp
        );

        let new_order_id = client
            .edit_order(
                order_id_string,
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

        // Update the order ID inside the Mutex with the new value
        let mut updated_order_id = edit_order_id.lock().await; // Lock the Mutex again to modify the value
        *updated_order_id = new_order_id.clone(); // Update the order ID inside the Mutex

        Ok(())
    }

    // if let Err(e) = edit_order(&client, edit_order_id, 0.23).await {
    //     error!("Error submitting order: {}", e);
    // }

    let client_clone = client.clone();

    let msg_read_handle = tokio::spawn(async move {
        let _ = client_clone
            .read_messages(tx)
            .await
            .map_err(|e| error!("Read messages error: {}", e));
    });

    client
        .subscribe_book_ticker("POPCAT".to_string(), "PERPETUAL".to_string())
        .await
        .unwrap();

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

                    // Call the edit_order function with the new bid price
                    let client_clone = Arc::clone(&client);
                    let edit_order_id_clone = Arc::clone(&edit_order_id);

                    tokio::spawn(async move {
                        if let Err(e) =
                            edit_order(&client_clone, edit_order_id_clone, adjusted_bid_px).await
                        {
                            error!("Error editing order: {}", e);
                        }
                    });
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
                    // Code without using `id`
                },
                
                _ => {}

            }

            
        }
    });

    // join!(msg_read_handle, msg_process_handle);
}
