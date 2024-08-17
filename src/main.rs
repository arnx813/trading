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
use tokio::{join, sync::mpsc};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
pub async fn main() {
    env_logger::init();
    dotenv().ok();

    let (tx, mut rx) = mpsc::unbounded_channel::<WsResponse>();

    let signing_key = std::env::var("SIGNING_KEY").unwrap();
    let wallet_address = std::env::var("WALLET_ADDRESS").unwrap();
    let api_key = std::env::var("API_KEY").unwrap();
    let api_secret = std::env::var("API_SECRET").unwrap();

    // Log the credentials (for debugging purposes only)
    info!("Signing Key: {}", signing_key);
    info!("Wallet Address: {}", wallet_address);
    info!("API Key: {}", api_key);
    info!("API Secret: {}", api_secret);

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
        info!("Testing submit_order function");

        let instrument_id = 34510; // Replace with actual instrument ID
        let is_buy = false; // Example: true for buy order
        let limit_price: f64 = 3.5; // Ensure this is f64
        let quantity: f64 = 5.0; // Ensure this is f64

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

    if let Err(e) = submit_order(&client).await {
        error!("Error submitting order: {}", e);
    }

    let client_clone = client.clone();

    let msg_read_handle = tokio::spawn(async move {
        let _ = client_clone
            .read_messages(tx)
            .await
            .map_err(|e| error!("Read messages error: {}", e));
    });

    client
        .subscribe_book_ticker("ZRO".to_string(), "PERPETUAL".to_string())
        .await
        .unwrap();

    // let msg_process_handle = tokio::spawn(async move {
    //     loop {
    //         let msg = rx.recv().await;
    //         match msg {
    //             Some(WsResponse::SubscribeResponse {
    //                 data: WsResponseData::BookTickerData { timestamp, tickers },
    //                 ..
    //             }) => {
    //                 let ticker = &tickers[0];
    //                 let (bid_px, ask_px): (f64, f64) = (
    //                     ticker.bid.price.parse().unwrap(),
    //                     ticker.ask.price.parse().unwrap(),   
    //                 );

    //                 let spread = (ask_px - bid_px) * 2.0 / (ask_px + bid_px);

    //                 info!(
    //                     "The lowest ask: {}; The highest bid: {}; The spread: {}",
    //                     ask_px, bid_px, spread
    //                 );
    //             }
    //             _ => {}
    //         }
    //     }
    // });

    // join!(msg_read_handle, msg_process_handle);
}
