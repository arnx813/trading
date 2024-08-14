use aevo_rust_sdk::{aevo::{self, ClientCredentials}, env::ENV}; 
use futures::{ SinkExt, StreamExt };
use tokio::{join, sync::mpsc}; 
use reqwest;
use tokio_tungstenite::tungstenite::Message; 
use std::{self, sync::Arc}; 
use dotenv::dotenv; 




#[tokio::main]
pub async fn main() {
    dotenv().ok();

    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    let credentials = ClientCredentials {
        signing_key : std::env::var("SIGNING_KEY").unwrap(), 
        wallet_address : std::env::var("WALLET_ADDRESS").unwrap(), 
        wallet_private_key : None, 
        api_key : std::env::var("API_KEY").unwrap(), 
        api_secret : std::env::var("API_SECRET").unwrap()
    }; 

    let client = Arc::new(aevo::AevoClient::new(
        Some(credentials), 
        ENV::MAINNET
    ).await.unwrap()); 

    let client_clone = client.clone(); 

    let msg_read_handle = tokio::spawn( async move {
        client_clone.read_messages(tx).await.unwrap(); 
    });  

    client.subscribe_markprice("ETH".to_string()).await.unwrap();

    let msg_process_handle = tokio::spawn( async move {
        loop {
            let msg = rx.recv().await; 
            match msg {
                Some(data) => println!("The data: {:?}", data), 
                None => {}
            }
        }
    });

    join!(msg_read_handle, msg_process_handle); 

}
