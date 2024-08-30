use aevo_hl_arb::XArb;
use aevo_hl_xmm::XMM;
use aevo_rust_sdk::aevo::{AevoClient, ClientCredentials};
use dotenv::dotenv;
mod aevo_hl_arb;
mod aevo_hl_xmm;

pub struct HLCredentials {
    pub api_key: String,
    pub api_wallet_address: String,
}

#[tokio::main]
pub async fn main() {
    env_logger::init();
    dotenv().ok();
    let aevo_credentials = ClientCredentials {
        signing_key: std::env::var("SIGNING_KEY").unwrap(),
        wallet_address: std::env::var("WALLET_ADDRESS").unwrap(),
        api_secret: std::env::var("API_SECRET").unwrap(),
        api_key: std::env::var("API_KEY").unwrap(),
        wallet_private_key: None,
    };

    let hl_credentials = HLCredentials {
        api_key: std::env::var("HL_API_PRIVATE_KEY").unwrap(),
        api_wallet_address: std::env::var("HL_API_WALLET_ADDRESS").unwrap(),
    };

    let mut cross_arb = XArb::new(
        aevo_credentials,
        hl_credentials,
        "POPCAT".to_string(),
        100.0,
        36426,
        10,
    )
    .await;

    cross_arb.start().await;
}

#[cfg(test)]
mod tests {
    use aevo_rust_sdk::{
        aevo::{AevoClient, ClientCredentials},
        env,
        rest::{OrderData, RestResponse},
        ws_structs::{WsResponse, WsResponseData},
    };
    use chrono::prelude::*;
    use dotenv::dotenv;
    use env_logger;
    use ethers::signers::LocalWallet;
    use hyperliquid_rust_sdk::{
        ClientLimit, ClientOrder, ClientOrderRequest, ExchangeClient, InfoClient, Message,
        Subscription,
    };
    use log::{error, info};
    use std::{sync::Arc, time::Duration};
    use tokio::{
        sync::{broadcast::error, mpsc},
        test,
        time::sleep,
    };

    #[test]
    async fn test_aevo_rest_open_order() {
        env_logger::init();
        dotenv().ok();
        let credentials = ClientCredentials {
            signing_key: std::env::var("SIGNING_KEY").unwrap(),
            wallet_address: std::env::var("WALLET_ADDRESS").unwrap(),
            api_secret: std::env::var("API_SECRET").unwrap(),
            api_key: std::env::var("API_KEY").unwrap(),
            wallet_private_key: None,
        };

        let mut client = AevoClient::new(Some(credentials), env::ENV::MAINNET)
            .await
            .unwrap();

        let time_before = Utc::now().timestamp_micros();
        let response = client
            .rest_create_order(1, true, 2400.0, 0.01, None, None)
            .await
            .unwrap();
        let time_after = Utc::now().timestamp_micros();

        println!("Response: {:?}", response);

        match response {
            RestResponse::CreateOrder { .. } => {
                println!(
                    "The rest order response latency is : {}",
                    time_after - time_before
                );
            }
            _ => {
                panic!("Not CreateOrder type: {:?}", response)
            }
        }

        let time_before = Utc::now().timestamp_micros();
        let response = client
            .rest_create_order(1, true, 2400.0, 0.01, None, None)
            .await
            .unwrap();
        let time_after = Utc::now().timestamp_micros();

        println!("Response: {:?}", response);

        match response {
            RestResponse::CreateOrder { .. } => {
                println!(
                    "The rest order response latency is : {}",
                    time_after - time_before
                );
            }
            _ => {
                panic!("Not CreateOrder type: {:?}", response)
            }
        }

        sleep(Duration::from_secs(60)).await;

        let time_before = Utc::now().timestamp_micros();
        let response = client
            .rest_create_order(1, true, 2400.0, 0.01, None, None)
            .await
            .unwrap();
        let time_after = Utc::now().timestamp_micros();

        println!("Response: {:?}", response);

        match response {
            RestResponse::CreateOrder { .. } => {
                println!(
                    "The rest order response latency is : {}",
                    time_after - time_before
                );
            }
            _ => {
                panic!("Not CreateOrder type: {:?}", response)
            }
        }
    }

    #[test]
    async fn test_aevo_rest_open_market_order() {
        env_logger::init();
        dotenv().ok();
        let credentials = ClientCredentials {
            signing_key: std::env::var("SIGNING_KEY").unwrap(),
            wallet_address: std::env::var("WALLET_ADDRESS").unwrap(),
            api_secret: std::env::var("API_SECRET").unwrap(),
            api_key: std::env::var("API_KEY").unwrap(),
            wallet_private_key: None,
        };

        let mut client = AevoClient::new(Some(credentials), env::ENV::MAINNET)
            .await
            .unwrap();

        let time_before = Utc::now().timestamp_micros();
        let response = client
            .rest_create_market_order(1, true, 0.01)
            .await
            .unwrap();
        let time_after = Utc::now().timestamp_micros();

        println!("Response: {:?}", response);

        match response {
            RestResponse::CreateOrder { .. } => {
                println!(
                    "The rest order response latency is : {}",
                    time_after - time_before
                );
            }
            _ => {
                panic!("Not CreateOrder type: {:?}", response)
            }
        }

        let time_before = Utc::now().timestamp_micros();
        let response = client
            .rest_create_market_order(1, false, 0.01)
            .await
            .unwrap();
        let time_after = Utc::now().timestamp_micros();

        println!("Response: {:?}", response);

        match response {
            RestResponse::CreateOrder { .. } => {
                println!(
                    "The rest order response latency is : {}",
                    time_after - time_before
                );
            }
            _ => {
                panic!("Not CreateOrder type: {:?}", response)
            }
        }
    }

    #[test]
    async fn test_aevo_rest_edit_order() {
        env_logger::init();
        dotenv().ok();
        let credentials = ClientCredentials {
            signing_key: std::env::var("SIGNING_KEY").unwrap(),
            wallet_address: std::env::var("WALLET_ADDRESS").unwrap(),
            api_secret: std::env::var("API_SECRET").unwrap(),
            api_key: std::env::var("API_KEY").unwrap(),
            wallet_private_key: None,
        };

        let mut client = Arc::new(
            AevoClient::new(Some(credentials), env::ENV::MAINNET)
                .await
                .unwrap(),
        );
        client.subscribe_fills().await.unwrap();

        let time_before = Utc::now().timestamp_micros();
        let order_id =
            "0x894258dbff96e94e874359d525e7e5423501accd764d931a697852fe5ab3ece6".to_string();
        let response = client
            .rest_edit_order(&order_id, 1, true, 2758.0, 0.02, None, None)
            .await
            .unwrap();
        let time_after = Utc::now().timestamp_micros();

        println!("Response: {:?}", response);

        match response {
            RestResponse::EditOrder { .. } => {
                println!(
                    "The rest order response latency is : {}",
                    time_after - time_before
                );
            }
            _ => {
                panic!("Not CreateOrder type: {:?}", response)
            }
        }

        let (aevo_tx, mut aevo_rx) = mpsc::unbounded_channel::<WsResponse>();
        let aevo_client = client.clone();
        tokio::spawn(async move {
            let _ = aevo_client
                .read_messages(aevo_tx)
                .await
                .map_err(|e| error!("Read messages error: {}", e));
        });

        loop {
            let msg = aevo_rx.recv().await;

            match msg {
                Some(WsResponse::SubscribeResponse {
                    data: WsResponseData::FillsData { timestamp, fill },
                    ..
                }) => {
                    info!("The fill response : {:?}", fill)
                }
                _ => {}
            }
        }
    }

    #[test]
    async fn test_ws_open_order() {
        env_logger::init();
        dotenv().ok();
        let (aevo_tx, mut aevo_rx) = mpsc::unbounded_channel::<WsResponse>();
        let credentials = ClientCredentials {
            signing_key: std::env::var("SIGNING_KEY").unwrap(),
            wallet_address: std::env::var("WALLET_ADDRESS").unwrap(),
            api_secret: std::env::var("API_SECRET").unwrap(),
            api_key: std::env::var("API_KEY").unwrap(),
            wallet_private_key: None,
        };

        let mut aevo_client = Arc::new(
            AevoClient::new(Some(credentials), env::ENV::MAINNET)
                .await
                .unwrap(),
        );

        let aevo_client_clone = aevo_client.clone();
        tokio::spawn(async move {
            let _ = aevo_client_clone
                .read_messages(aevo_tx)
                .await
                .map_err(|e| error!("Read messages error: {}", e));
        });

        let time_before = Utc::now().timestamp_micros();
        let order_id = aevo_client
            .create_order(1, true, 2400.0, 0.01, None, None, None)
            .await
            .unwrap();
        let time_after = Utc::now().timestamp_micros();

        println!(
            "The ws order send latency is : {}",
            time_after - time_before
        );

        loop {
            let msg = aevo_rx.recv().await;
            match msg {
                Some(WsResponse::PublishResponse {
                    data:
                        WsResponseData::CreateEditOrderData {
                            order_id,
                            timestamp,
                            ..
                        },
                    ..
                }) => {
                    let time_rn = Utc::now().timestamp_micros();
                    let timestamp_micros = timestamp.parse::<i64>().unwrap() / 1000;
                    println!(
                        "The aevo reach latency is : {}",
                        timestamp_micros - time_after
                    );
                    println!(
                        "The latency from aevo to os : {}",
                        time_rn - timestamp_micros
                    );
                    println!(
                        "The total latency from sending to receiving response : {}",
                        time_rn - time_before
                    );
                    break;
                }
                _ => {}
            }
        }
    }

    #[test]
    async fn test_hl_rest_open_order() {
        env_logger::init();
        dotenv().ok();

        let hl_signer: LocalWallet = std::env::var("HL_API_PRIVATE_KEY")
            .unwrap()
            .parse()
            .unwrap();
        let mut client = ExchangeClient::new(None, hl_signer, None, None, None)
            .await
            .unwrap();

        let time_before = Utc::now().timestamp_micros();
        let response = client
            .order(
                ClientOrderRequest {
                    asset: "POPCAT".to_string(),
                    is_buy: true,
                    reduce_only: false,
                    limit_px: 0.75,
                    sz: 100.0,
                    cloid: None,
                    order_type: ClientOrder::Limit(ClientLimit {
                        tif: "Ioc".to_string(), // market order is agressive limit order with IOC TIF
                    }),
                },
                None,
            )
            .await;
        let time_after = Utc::now().timestamp_micros();

        println!("Response: {:?}", response);
        println!(
            "The rest order creation latency is : {}",
            time_after - time_before
        );
    }

    #[test]
    async fn test_aevo_rest_open_hl_open() {
        env_logger::init();
        dotenv().ok();

        let (aevo_tx, mut aevo_rx) = mpsc::unbounded_channel::<WsResponse>();

        let credentials = ClientCredentials {
            signing_key: std::env::var("SIGNING_KEY").unwrap(),
            wallet_address: std::env::var("WALLET_ADDRESS").unwrap(),
            api_secret: std::env::var("API_SECRET").unwrap(),
            api_key: std::env::var("API_KEY").unwrap(),
            wallet_private_key: None,
        };

        let mut aevo_client = Arc::new(
            AevoClient::new(Some(credentials), env::ENV::MAINNET)
                .await
                .unwrap(),
        );

        let aevo_client_clone = aevo_client.clone();
        tokio::spawn(async move {
            let _ = aevo_client_clone
                .read_messages(aevo_tx)
                .await
                .map_err(|e| error!("Read messages error: {}", e));
        });
        aevo_client.subscribe_fills().await.unwrap();

        let hl_signer: LocalWallet = std::env::var("HL_API_PRIVATE_KEY")
            .unwrap()
            .parse()
            .unwrap();
        let mut hl_exchange_client = ExchangeClient::new(None, hl_signer, None, None, None)
            .await
            .unwrap();
        let mut hl_info_client = InfoClient::new(None, None).await.unwrap();

        let time_before = Utc::now().timestamp_micros();
        let response = aevo_client
            .rest_create_market_order(1, true, 0.01)
            .await
            .unwrap();
        let time_after = Utc::now().timestamp_micros();

        println!("Response: {:?}", response);
        println!(
            "The rest order creation latency is : {}",
            time_after - time_before
        );

        let aevo_order_id = match response {
            RestResponse::CreateOrder(OrderData { order_id, .. }) => order_id,
            _ => unreachable!(),
        };

        loop {
            let msg = aevo_rx.recv().await;
            match msg {
                Some(WsResponse::SubscribeResponse {
                    data: WsResponseData::FillsData { timestamp, fill },
                    ..
                }) => {
                    if fill.order_id == aevo_order_id {
                        info!("Filled size : {}, price : {}", fill.filled, fill.price);
                        let time_before = Utc::now().timestamp_micros();
                        let response = hl_exchange_client
                            .order(
                                ClientOrderRequest {
                                    asset: "ETH".to_string(),
                                    is_buy: false,
                                    reduce_only: false,
                                    limit_px: 2400.0,
                                    sz: 0.01,
                                    cloid: None,
                                    order_type: ClientOrder::Limit(ClientLimit {
                                        tif: "Ioc".to_string(), // market order is agressive limit order with IOC TIF
                                    }),
                                },
                                None,
                            )
                            .await;
                        let time_after = Utc::now().timestamp_micros();
                        println!("Response: {:?}", response);
                        println!(
                            "The rest order creation latency is : {}",
                            time_after - time_before
                        );
                        break;
                    }
                }
                _ => {}
            }
        }
    }
}
