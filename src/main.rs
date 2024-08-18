use aevo_rust_sdk::{
    aevo::{self, ClientCredentials},
    env::ENV,
    ws_structs::{Fill, WsResponse, WsResponseData},
};
use dotenv::dotenv;
use env_logger;
use ethers::signers::LocalWallet;
use futures::{SinkExt, StreamExt};
use hyperliquid_rust_sdk::{
    BaseUrl, ClientCancelRequest, ClientLimit, ClientOrder, ClientOrderRequest, ExchangeClient,
    ExchangeDataStatus, ExchangeResponseStatus, InfoClient, Message, Subscription,
};
use log::{error, info};
use reqwest;
use std::{self, sync::Arc};
use tokio::{join, sync::mpsc};

#[tokio::main]
pub async fn main() {
    env_logger::init();
    dotenv().ok();

    let signing_key = std::env::var("SIGNING_KEY").unwrap();

    // Log the credentials (for debugging purposes only)
    info!("Signing Key: {}", signing_key);

    let wallet: LocalWallet = signing_key
        .parse()
        .expect("Failed to parse wallet private key");

    let exchange_client = ExchangeClient::new(None, wallet, Some(BaseUrl::Testnet), None, None)
        .await
        .unwrap();

    let order = ClientOrderRequest {
        asset: "PURR/USDC".to_string(),
        is_buy: true,
        reduce_only: false,
        limit_px: 100.0,
        sz: 1.0,
        cloid: None,
        order_type: ClientOrder::Limit(ClientLimit {
            tif: "Gtc".to_string(),
        }),
    };

    let response = exchange_client.order(order, None).await.unwrap();
    info!("Order placed: {response:?}");

    let response = match response {
        ExchangeResponseStatus::Ok(exchange_response) => exchange_response,
        ExchangeResponseStatus::Err(e) => panic!("error with exchange response: {e}"),
    };
    let status = response.data.unwrap().statuses[0].clone();
    let oid = match status {
        ExchangeDataStatus::Filled(order) => order.oid,
        ExchangeDataStatus::Resting(order) => order.oid,
        _ => panic!("Error: {status:?}"),
    };

    // let (aevo_tx, mut aevo_rx) = mpsc::unbounded_channel::<WsResponse>();

    // let credentials = ClientCredentials {
    //     signing_key : std::env::var("SIGNING_KEY").unwrap(),
    //     wallet_address : std::env::var("WALLET_ADDRESS").unwrap(),
    //     wallet_private_key : None,
    //     api_key : std::env::var("API_KEY").unwrap(),
    //     api_secret : std::env::var("API_SECRET").unwrap()
    // };

    // let aevo_client = Arc::new(aevo::AevoClient::new(
    //     Some(credentials),
    //     ENV::MAINNET
    // ).await.unwrap());

    let mut hl_info_client = InfoClient::new(None, Some(BaseUrl::Mainnet)).await.unwrap();

    let (hl_tx, mut hl_rx) = mpsc::unbounded_channel::<Message>();

    hl_info_client
        .subscribe(Subscription::AllMids, hl_tx)
        .await
        .unwrap();

    // let client_clone = aevo_client.clone();

    // let msg_read_handle = tokio::spawn( async move {
    //     let _ = client_clone.read_messages(aevo_tx).await.map_err(|e| error!("Read messages error: {}", e));
    // });

    // aevo_client.subscribe_book_ticker("ZRO".to_string(), "PERPETUAL".to_string()).await.unwrap();

    let msg_process_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                // msg = aevo_rx.recv() => {
                //     match msg {
                //         Some(WsResponse::SubscribeResponse {
                //             data: WsResponseData::BookTickerData {
                //                 timestamp,
                //                 tickers
                //             },
                //             ..
                //         }) => {
                //             let ticker = &tickers[0];
                //             let (bid_px, ask_px): (f64, f64) = (ticker.bid.price.parse().unwrap(), ticker.ask.price.parse().unwrap());

                //             let spread = (ask_px - bid_px) * 2.0 / (ask_px + bid_px);

                //             info!("The lowest ask: {}; The highest bid: {}; The spread: {}", ask_px, bid_px, spread);
                //         },
                //         _ => {}
                //     }
                // },
                msg = hl_rx.recv() => {
                    match msg {
                        Some(Message::AllMids(all_mids)) => {
                            let zro_mid = all_mids.data.mids.get("ZRO");
                            info!("Hyperliquid All mids : {:?}", zro_mid);
                        },
                        _ => {}
                    }
                }

                // subtraction here
            }
        }
    });

    join!(msg_process_handle);
}
