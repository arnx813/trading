use aevo_rust_sdk::{
    aevo::{self, ClientCredentials},
    env::ENV,
    ws_structs::{Fill, WsResponse, WsResponseData},
};
use dotenv::dotenv;
use env_logger;
use csv;
use futures::{SinkExt, StreamExt};
use hyperliquid_rust_sdk::{BaseUrl, InfoClient, Message, Subscription};
use log::{error, info};
use reqwest;
use serde_derive::{Deserialize, Serialize};
use std::{self, sync::Arc};
use tokio::{
    join,
    sync::{mpsc, Mutex},
    time::{timeout, interval,  Duration}
};

#[derive(Serialize, Deserialize, Debug)]
pub struct CrossExchangeState {
    pub aevo_bid: f64,
    pub aevo_ask: f64,
    pub hl_bid: f64,
    pub hl_ask: f64,
}

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    dotenv().ok();

    let (aevo_tx, mut aevo_rx) = mpsc::unbounded_channel::<WsResponse>();

    let credentials = ClientCredentials {
        signing_key: std::env::var("SIGNING_KEY").unwrap(),
        wallet_address: std::env::var("WALLET_ADDRESS").unwrap(),
        wallet_private_key: None,
        api_key: std::env::var("API_KEY").unwrap(),
        api_secret: std::env::var("API_SECRET").unwrap(),
    };

    let aevo_client = Arc::new(
        aevo::AevoClient::new(Some(credentials), ENV::MAINNET)
            .await
            .unwrap(),
    );

    let mut hl_info_client = InfoClient::new(None, Some(BaseUrl::Mainnet)).await.unwrap();

    let (hl_tx, mut hl_rx) = mpsc::unbounded_channel::<Message>();

    hl_info_client
        .subscribe(
            Subscription::L2Book {
                coin: "POPCAT".to_string(),
            },
            hl_tx,
        )
        .await
        .unwrap();

    let client_clone = aevo_client.clone();

    let msg_read_handle = tokio::spawn(async move {
        let _ = client_clone
            .read_messages(aevo_tx)
            .await
            .map_err(|e| error!("Read messages error: {}", e));
    });

    aevo_client
        .subscribe_book_ticker("POPCAT".to_string(), "PERPETUAL".to_string())
        .await
        .unwrap();

    let mm_state = Arc::new(Mutex::new(CrossExchangeState {
        aevo_bid: -1.0,
        aevo_ask: -1.0,
        hl_bid: -1.0,
        hl_ask: -1.0,
    }));

    let mut writer = csv::Writer::from_path("mm_state.csv").unwrap(); 
    let mut flush_interval = interval(Duration::from_secs(60));

    let mm_state_clone = Arc::clone(&mm_state);

    let msg_process_handle = tokio::spawn(async move {
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

                            {
                                let mut state_guard = mm_state_clone.lock().await;
                                state_guard.aevo_ask = ask_px;
                                state_guard.aevo_bid = bid_px;
                                writer.serialize(&*state_guard).unwrap();
                            }

                            info!("The MM state : {:?}", mm_state_clone);
                        },
                        _ => {}
                    }
                },
                msg = hl_rx.recv() => {
                    match msg {
                        Some(Message::L2Book(l2_book)) => {
                            let l2_book = l2_book.data;
                            if l2_book.coin == "POPCAT" {
                                let (ask_px, bid_px): (f64, f64) = (l2_book.levels[1][0].px.parse().unwrap(), l2_book.levels[0][0].px.parse().unwrap());

                                let spread = (ask_px - bid_px) * 2.0 / (ask_px + bid_px);

                                {
                                    let mut state_guard = mm_state_clone.lock().await;
                                    state_guard.hl_ask = ask_px;
                                    state_guard.hl_bid = bid_px;
                                    writer.serialize(&*state_guard).unwrap();
                                }

                                info!("The MM state : {:?}", mm_state_clone);
                            }
                        },
                        _ => {}
                    }
                }, 
                _ = flush_interval.tick() => {
                    writer.flush().unwrap();
                    println!("CSV Flushed writer");
                },
            }
        }
    });

    // Join the tasks and await their completion
    let _ = join!(msg_read_handle, msg_process_handle);

    info!("MM State stored in DataFrame and written to mm_state.csv");

    Ok(())
}
