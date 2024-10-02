use std::sync::Arc;

use ethers::signers::{LocalWallet, Signer};
use futures_util::stream::SplitSink;
use hyperliquid_rust_sdk as HL;
use log::{error, info};
use tokio::{
    net::TcpStream,
    sync::{mpsc, Mutex},
};
use tokio_tungstenite::{tungstenite::protocol, MaybeTlsStream, WebSocketStream};

use crate::HLCredentials;

pub struct RestingOrder {
    state: OrderState,
    oid: u64,
    sz: f64,
    px: f64,
    filled: f64,
}

pub enum OrderState {
    Idle,
    InFlight {
        ws_request_id: u64,
    },
    Verified,
    Filled,
    ModifyInFlight {
        ws_request_id: u64,
        request: HL::ClientOrderRequest,
    },
    ModifyError,
}

pub struct MM {
    pub ask_px: f64,
    pub bid_px: f64,
    pub opened_sz: f64,
    pub buy_resting: RestingOrder,
    pub sell_resting: RestingOrder,
    pub hl_info_client: HL::InfoClient,
    pub hl_exchange_client: HL::ExchangeClient,
    pub wallet_address: String,
    pub min_spread_bps: u64,
    pub max_px_delta_bps: u64,
}

pub fn hex_str_to_u8_slice(hex_str: &str) -> Result<[u8; 20], String> {
    // Remove "0x" prefix if present
    let hex_str = hex_str.trim_start_matches("0x");

    // Ensure that the hex string has an even length
    if hex_str.len() != 40 {
        return Err("Hex string has wrong length".to_string());
    }

    let mut result = [0u8; 20];

    for i in 0..20 {
        result[i] = u8::from_str_radix(&hex_str[2 * i..2 * i + 2], 16).unwrap()
    }

    Ok(result)
}

impl MM {
    pub async fn new(
        hl_credentials: HLCredentials,
        min_spread_bps: u64,
        max_px_delta_bps: u64,
    ) -> MM {
        let hl_info_client = HL::InfoClient::new(None, None).await.unwrap();
        let hl_signer: LocalWallet = hl_credentials.api_key.parse().unwrap();
        let hl_exchange_client = HL::ExchangeClient::new(None, hl_signer, None, None, None)
            .await
            .unwrap();

        MM {
            ask_px: -1.0,
            bid_px: -1.0,
            opened_sz: 0.0,
            buy_resting: RestingOrder {
                state: OrderState::Idle,
                oid: 0,
                sz: 0.0,
                px: -1.0,
                filled: 0.0,
            },
            sell_resting: RestingOrder {
                state: OrderState::Idle,
                oid: 0,
                sz: 0.0,
                px: -1.0,
                filled: 0.0,
            },
            hl_info_client,
            hl_exchange_client,
            wallet_address: hl_credentials.wallet_address,
            min_spread_bps,
            max_px_delta_bps,
        }
    }

    pub async fn start(&mut self, asset: String, max_size: f64, decimals: u32) {
        let (tx, mut rx) = mpsc::unbounded_channel::<HL::Message>();

        self.hl_info_client
            .subscribe(
                HL::Subscription::L2Book {
                    coin: asset.clone(),
                },
                tx.clone(),
            )
            .await
            .unwrap();

        self.hl_info_client
            .subscribe(
                HL::Subscription::UserEvents {
                    user: hex_str_to_u8_slice(&self.wallet_address).unwrap().into(),
                },
                tx.clone(),
            )
            .await
            .unwrap();

        self.hl_info_client
            .subscribe(HL::Subscription::Post, tx.clone())
            .await
            .unwrap();

        loop {
            let msg = rx.recv().await;
            if let Some(msg) = msg {
                match msg {
                    HL::Message::L2Book(l2_book) => {
                        let l2_book = l2_book.data;
                        self.ask_px = l2_book.levels[1][0].px.parse().unwrap();
                        self.bid_px = l2_book.levels[0][0].px.parse().unwrap();

                        let writer = match &self.hl_info_client.ws_manager {
                            Some(manager) => manager.writer.clone(),
                            None => panic!(),
                        };

                        self.ws_submit_orders(writer, &asset, max_size, decimals)
                            .await;
                    }
                    HL::Message::User(user_events) => {
                        let user_events = user_events.data;
                        if let HL::UserData::Fills(fills) = user_events {
                            for fill in fills {
                                self.process_fill(&fill);
                            }
                        }
                    }
                    HL::Message::Post(HL::PostResponse {
                        data: HL::PostResponseData { id, response },
                    }) => {
                        if let HL::Response::Action {
                            payload: response_status,
                        } = &response
                        {
                            self.process_post_response_status(response_status, id);
                        }
                    }
                    _ => unimplemented!(),
                };
            }
        }
    }

    pub fn process_fill(&mut self, fill: &HL::TradeInfo) {
        let sz: f64 = fill.sz.parse().unwrap();

        if fill.oid.eq(&self.buy_resting.oid) {
            let buy_resting = &mut self.buy_resting;
            match buy_resting.state {
                OrderState::Verified => {
                    self.opened_sz += sz;
                    buy_resting.filled += sz;
                    if buy_resting.filled.eq(&buy_resting.sz) {
                        buy_resting.state = OrderState::Filled;
                    }
                    info!(
                        "BUY FILL: order filled (filled: {sz}, total filled: {}, total sz: {})",
                        buy_resting.filled, buy_resting.sz
                    );
                }
                OrderState::ModifyError => {
                    self.opened_sz += sz;
                    buy_resting.filled += sz;
                    if buy_resting.filled.eq(&buy_resting.sz) {
                        buy_resting.state = OrderState::Filled;
                    } else {
                        buy_resting.state = OrderState::Verified;
                    }
                    info!(
                        "BUY FILL: order filled (filled: {sz}, total filled: {}, total sz: {})",
                        buy_resting.filled, buy_resting.sz
                    );
                }
                OrderState::ModifyInFlight { .. } => {
                    self.opened_sz += sz;
                    buy_resting.filled += sz;
                    info!(
                        "BUY FILL: order filled (filled: {sz}, total filled: {}, total sz: {})",
                        buy_resting.filled, buy_resting.sz
                    );
                }
                _ => unreachable!("FILL: not expected state: {fill:?}"),
            }
        } else if fill.oid.eq(&self.sell_resting.oid) {
            let sell_resting = &mut self.sell_resting;
            match sell_resting.state {
                OrderState::Verified => {
                    self.opened_sz -= sz;
                    sell_resting.filled += sz;
                    if sell_resting.filled.eq(&sell_resting.sz) {
                        sell_resting.state = OrderState::Filled;
                    }
                    info!(
                        "SELL FILL: order filled (filled: {sz}, total filled: {}, total sz: {})",
                        sell_resting.filled, sell_resting.sz
                    );
                }
                OrderState::ModifyError => {
                    self.opened_sz -= sz;
                    sell_resting.filled += sz;
                    if sell_resting.filled.eq(&sell_resting.sz) {
                        sell_resting.state = OrderState::Filled;
                    } else {
                        sell_resting.state = OrderState::Verified;
                    }
                    info!(
                        "SELL FILL: order filled (filled: {sz}, total filled: {}, total sz: {})",
                        sell_resting.filled, sell_resting.sz
                    );
                }
                OrderState::ModifyInFlight { .. } => {
                    self.opened_sz -= sz;
                    sell_resting.filled += sz;
                    info!(
                        "SELL FILL: order filled (filled: {sz}, total filled: {}, total sz: {})",
                        sell_resting.filled, sell_resting.sz
                    );
                }
                _ => unreachable!("FILL: not expected state: {fill:?}"),
            }
        } else {
            panic!("FILL: oid does not satisfy any of the resting orders: {fill:?}")
        }
    }

    pub fn process_post_response_status(
        &mut self,
        response_status: &HL::ExchangeResponseStatus,
        ws_id: u64,
    ) {
        match response_status {
            HL::ExchangeResponseStatus::Ok(order) => {
                if let Some(order) = &order.data {
                    let data_status = &order.statuses[0];
                    assert_eq!(order.statuses.len(), 1);
                    self.process_post_data_status(data_status, ws_id);
                } else {
                    error!("Exchange response data is empty when placing order : {order:?}");
                }
            }
            HL::ExchangeResponseStatus::Err(e) => {
                error!("Error with placing/modifying order {e}")
            }
        }
    }

    pub fn process_post_data_status(&mut self, data_status: &HL::ExchangeDataStatus, ws_id: u64) {
        match &self.buy_resting.state {
            OrderState::ModifyInFlight {
                ws_request_id,
                request,
            } => {
                if ws_request_id.eq(&ws_id) {
                    match data_status {
                        HL::ExchangeDataStatus::Filled(order) => {
                            info!("BUY: order modified and filled");
                            self.buy_resting = RestingOrder {
                                state: OrderState::Verified,
                                oid: order.oid,
                                sz: request.sz,
                                px: request.limit_px,
                                filled: 0.0,
                            };
                        }
                        HL::ExchangeDataStatus::Resting(order) => {
                            info!("BUY: order modified and resting");
                            self.buy_resting = RestingOrder {
                                state: OrderState::Verified,
                                oid: order.oid,
                                sz: request.sz,
                                px: request.limit_px,
                                filled: 0.0,
                            };
                        }
                        HL::ExchangeDataStatus::Error(e) => {
                            self.buy_resting.state =
                                if self.buy_resting.filled.eq(&self.buy_resting.sz) {
                                    error!("BUY: order modify error: {e}, filled already");
                                    OrderState::Filled
                                } else {
                                    error!("BUY: order modify error: {e}, awaiting fill");
                                    OrderState::ModifyError
                                };
                        }
                        _ => unreachable!(),
                    }
                }
            }
            OrderState::InFlight { ws_request_id } => {
                if ws_request_id.eq(&ws_id) {
                    match data_status {
                        HL::ExchangeDataStatus::Filled(order) => {
                            info!("BUY: order filled placed");
                            self.buy_resting.state = OrderState::Verified;
                            self.buy_resting.oid = order.oid;
                        }
                        HL::ExchangeDataStatus::Resting(order) => {
                            info!("BUY: order resting placed");
                            self.buy_resting.state = OrderState::Verified;
                            self.buy_resting.oid = order.oid;
                        }
                        HL::ExchangeDataStatus::Error(e) => {
                            self.buy_resting.state = OrderState::Idle;
                            error!("BUY: order place error: {e}");
                        }
                        _ => unreachable!(),
                    }
                }
            }
            OrderState::Filled
            | OrderState::Idle
            | OrderState::Verified
            | OrderState::ModifyError => {
                // do nothing -> not expected to get any post responses if no post data in flight
            }
        }

        match &self.sell_resting.state {
            OrderState::ModifyInFlight {
                ws_request_id,
                request,
            } => {
                if ws_request_id.eq(&ws_id) {
                    match data_status {
                        HL::ExchangeDataStatus::Filled(order) => {
                            info!("SELL: order modified and filled");
                            self.sell_resting = RestingOrder {
                                state: OrderState::Verified,
                                oid: order.oid,
                                sz: request.sz,
                                px: request.limit_px,
                                filled: 0.0,
                            };
                        }
                        HL::ExchangeDataStatus::Resting(order) => {
                            info!("SELL: order modified and resting");
                            self.sell_resting = RestingOrder {
                                state: OrderState::Verified,
                                oid: order.oid,
                                sz: request.sz,
                                px: request.limit_px,
                                filled: 0.0,
                            };
                        }
                        HL::ExchangeDataStatus::Error(e) => {
                            self.sell_resting.state =
                                if self.sell_resting.filled.eq(&self.sell_resting.sz) {
                                    error!("SELL: order modify error: {e}, filled already");
                                    OrderState::Filled
                                } else {
                                    error!("SELL: order modify error: {e}, awaiting fill");
                                    OrderState::ModifyError
                                };
                        }
                        _ => unreachable!(),
                    }
                }
            }
            OrderState::InFlight { ws_request_id } => {
                if ws_request_id.eq(&ws_id) {
                    match data_status {
                        HL::ExchangeDataStatus::Filled(order) => {
                            info!("SELL: order filled placed");
                            self.sell_resting.state = OrderState::Verified;
                            self.sell_resting.oid = order.oid;
                        }
                        HL::ExchangeDataStatus::Resting(order) => {
                            info!("SELL: order resting placed");
                            self.sell_resting.state = OrderState::Verified;
                            self.sell_resting.oid = order.oid;
                        }
                        HL::ExchangeDataStatus::Error(e) => {
                            self.sell_resting.state = OrderState::Idle;
                            error!("SELL: order place error: {e}");
                        }
                        _ => unreachable!(),
                    }
                }
            }
            OrderState::Filled
            | OrderState::Idle
            | OrderState::Verified
            | OrderState::ModifyError => {
                // do nothing -> not expected to get any post responses if no post data in flight
            }
        }
    }

    pub async fn ws_submit_orders(
        &mut self,
        writer: Arc<
            Mutex<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, protocol::Message>>,
        >,
        asset: &str,
        max_size: f64,
        decimals: u32,
    ) {
        let mid_price = self.bid_px + (self.ask_px - self.bid_px) / 2.0;
        let spread = self.ask_px - self.bid_px;
        let spread_bps = 10000.0 * spread / mid_price;
        let min_spread = self.min_spread_bps as f64 * mid_price / 10000.0;

        info!(
            "L2_BOOK: ask_px: {}, bid_px: {}, spread_bps: {}",
            self.ask_px, self.bid_px, spread_bps
        );
        let buy_sz = (max_size - self.opened_sz).max(0.0).min(max_size);
        let sell_sz = (self.opened_sz + max_size).max(0.0).min(max_size);

        let buy_px = MM::truncate_float(
            self.bid_px.min(mid_price - min_spread / 2.0),
            decimals,
            false,
        );
        let sell_px = MM::truncate_float(
            self.ask_px.max(mid_price + min_spread / 2.0),
            decimals,
            true,
        );

        if buy_sz > 0.0 {
            let buy_request = HL::ClientOrderRequest {
                asset: asset.to_string(),
                is_buy: true,
                reduce_only: false,
                limit_px: buy_px,
                sz: buy_sz,
                cloid: None,
                order_type: HL::ClientOrder::Limit(HL::ClientLimit {
                    tif: "Gtc".to_string(), // good til cancel order type
                }),
            };
            match self.buy_resting.state {
                OrderState::Idle | OrderState::Filled => {
                    let send_status = self
                        .hl_exchange_client
                        .ws_order(buy_request, None, writer.clone())
                        .await;
                    self.buy_resting = RestingOrder {
                        sz: buy_sz,
                        px: buy_px,
                        state: match send_status {
                            Ok(id) => {
                                info!("BUY: order sent (id: {id}, sz: {buy_sz}, px: {buy_px})");
                                OrderState::InFlight { ws_request_id: id }
                            }
                            Err(e) => {
                                error!("BUY: order send error: {e}");
                                OrderState::Idle
                            }
                        },
                        oid: 0,
                        filled: 0.0,
                    };
                }
                OrderState::Verified => {
                    let diff_bps = ((buy_px - self.buy_resting.px).abs() * 10000.0 / buy_px) as u64;

                    if diff_bps > self.max_px_delta_bps {
                        let modify_request = HL::ClientModifyRequest {
                            oid: self.buy_resting.oid,
                            order: buy_request.clone(),
                        };

                        let send_status = self
                            .hl_exchange_client
                            .ws_modify(modify_request, None, writer.clone())
                            .await;
                        match send_status {
                            Ok(id) => {
                                info!("BUY: order modify sent (id: {id})");
                                self.buy_resting.state = OrderState::ModifyInFlight {
                                    ws_request_id: id,
                                    request: buy_request,
                                };
                            }
                            Err(e) => {
                                error!("BUY: modify send error: {e}");
                            }
                        }
                    }
                }
                OrderState::InFlight { ws_request_id } => {
                    info!("BUY: order in flight (id: {ws_request_id})")
                }
                OrderState::ModifyInFlight { ws_request_id, .. } => {
                    info!("BUY: modify in flight (id : {ws_request_id})")
                }
                OrderState::ModifyError => {
                    info!("BUY: order in modify error state awaiting fill")
                }
            }
        }

        if sell_sz > 0.0 {
            let sell_request = HL::ClientOrderRequest {
                asset: asset.to_string(),
                is_buy: false,
                reduce_only: false,
                limit_px: sell_px,
                sz: sell_sz,
                cloid: None,
                order_type: HL::ClientOrder::Limit(HL::ClientLimit {
                    tif: "Gtc".to_string(), // good til cancel order type
                }),
            };
            match self.sell_resting.state {
                OrderState::Idle | OrderState::Filled => {
                    let send_status = self
                        .hl_exchange_client
                        .ws_order(sell_request, None, writer.clone())
                        .await;
                    self.sell_resting = RestingOrder {
                        sz: sell_sz,
                        px: sell_px,
                        state: match send_status {
                            Ok(id) => {
                                info!("SELL: order sent (id: {id}, sz: {sell_sz}, px: {sell_px})");
                                OrderState::InFlight { ws_request_id: id }
                            }
                            Err(e) => {
                                error!("SELL: order send error: {e}");
                                OrderState::Idle
                            }
                        },
                        oid: 0,
                        filled: 0.0,
                    };
                }
                OrderState::Verified => {
                    let diff_bps =
                        ((sell_px - self.sell_resting.px).abs() * 10000.0 / sell_px) as u64;

                    if diff_bps > self.max_px_delta_bps {
                        let modify_request = HL::ClientModifyRequest {
                            oid: self.sell_resting.oid,
                            order: sell_request.clone(),
                        };
                        let send_status = self
                            .hl_exchange_client
                            .ws_modify(modify_request, None, writer.clone())
                            .await;
                        match send_status {
                            Ok(id) => {
                                info!("SELL: order modify sent (id: {id})");
                                self.sell_resting.state = OrderState::ModifyInFlight {
                                    ws_request_id: id,
                                    request: sell_request,
                                }
                            }
                            Err(e) => {
                                error!("SELL: modify send error: {e}");
                            }
                        }
                    }
                }
                OrderState::InFlight { ws_request_id } => {
                    info!("SELL: order in flight (id: {ws_request_id})")
                }
                OrderState::ModifyInFlight { ws_request_id, .. } => {
                    info!("SELL: cancel in flight (id : {ws_request_id})")
                }
                OrderState::ModifyError => {
                    info!("SELL: order in modify error state awaiting fill")
                }
            }
        }
    }

    pub fn truncate_float(float: f64, decimals: u32, round_up: bool) -> f64 {
        let pow10 = 10i64.pow(decimals) as f64;
        let mut float = (float * pow10) as u64;
        if round_up {
            float += 1;
        }
        float as f64 / pow10
    }
}
