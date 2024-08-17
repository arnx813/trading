use aevo_rust_sdk::{aevo::{self, ClientCredentials}, env::ENV, ws_structs::{Fill, WsResponse, WsResponseData}}; 
use futures::{ SinkExt, StreamExt };
use log::{info, error};
use env_logger; 
use tokio::{join, sync::mpsc}; 
use reqwest;
use std::{self, sync::Arc}; 
use dotenv::dotenv; 
use hyperliquid_rust_sdk::{BaseUrl, InfoClient, Message, Subscription}; 




#[tokio::main]
pub async fn main() {
    env_logger::init();
    dotenv().ok();

    let (aevo_tx, mut aevo_rx) = mpsc::unbounded_channel::<WsResponse>();

    let credentials = ClientCredentials {
        signing_key : std::env::var("SIGNING_KEY").unwrap(), 
        wallet_address : std::env::var("WALLET_ADDRESS").unwrap(), 
        wallet_private_key : None, 
        api_key : std::env::var("API_KEY").unwrap(), 
        api_secret : std::env::var("API_SECRET").unwrap()
    }; 

    let aevo_client = Arc::new(aevo::AevoClient::new(
        Some(credentials), 
        ENV::MAINNET
    ).await.unwrap()); 

    let mut hl_info_client = InfoClient::new(None, Some(BaseUrl::Mainnet)).await.unwrap(); 

    let (hl_tx, mut hl_rx) = mpsc::unbounded_channel::<Message>();

    hl_info_client.subscribe(Subscription::AllMids, hl_tx).await.unwrap(); 

    let client_clone = aevo_client.clone(); 

    let msg_read_handle = tokio::spawn( async move {
        let _ = client_clone.read_messages(aevo_tx).await.map_err(|e| error!("Read messages error: {}", e)); 
    });  

    aevo_client.subscribe_book_ticker("ZRO".to_string(), "PERPETUAL".to_string()).await.unwrap();

    let msg_process_handle = tokio::spawn( async move {
        loop {
            tokio::select! {
                msg = aevo_rx.recv() => {
                    match msg {
                        Some(WsResponse::SubscribeResponse {
                            data: WsResponseData::BookTickerData { 
                                timestamp, 
                                tickers 
                            } , 
                            ..
                        }) => {
                            let ticker = &tickers[0]; 
                            let (bid_px, ask_px): (f64, f64) = (ticker.bid.price.parse().unwrap(), ticker.ask.price.parse().unwrap());
        
                            let spread = (ask_px - bid_px) * 2.0 / (ask_px + bid_px); 
        
                            info!("The lowest ask: {}; The highest bid: {}; The spread: {}", ask_px, bid_px, spread); 
        
                        },
                        _ => {}
                    }
                }, 
                msg = hl_rx.recv() => {
                    match msg {
                        Some(Message::AllMids(all_mids)) => {
                            let all_mids = all_mids.data.mids;
                            info!("Hyperliquid All mids : {:?}", all_mids); 
                        }, 
                        _ => {}
                    }
                }
            }
        }
    });

    join!(msg_read_handle, msg_process_handle); 

}
