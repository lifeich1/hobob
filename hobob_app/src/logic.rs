use super::*;
use bevy::utils::Duration;
use bilibili_api_rs::plugin::{ApiRequestEvent, ApiTaskResultEvent};
use chrono::naive::NaiveDateTime;
use serde_json::json;
use std::collections::VecDeque;
use ui::following::{
    data::{self, Data},
    event::ParsedApiResult,
};

mod backend;
mod frontend;

pub struct LogicPlugin();

impl Plugin for LogicPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.add_plugin(frontend::input::ModPlugin())
            .add_plugin(frontend::display::ModPlugin())
            .add_plugin(backend::ModPlugin())

            // backend::timer
            .insert_resource(AutoRefreshTimer::default())
            .add_system(periodly_refresh_all.system())
            // backend::parser
            .add_system(first_parse_api_result.system())
            .add_system(sort_key_api_result.system())
            // backend::face
            ;
    }
}

struct AutoRefreshTimer {
    timer: Timer,
    queue: VecDeque<u64>,
}

impl AutoRefreshTimer {
    fn refill(&mut self, cf: &Res<AppConfig>) -> &mut Self {
        if self.queue.is_empty() {
            self.queue.extend(cf.followings_uid.clone());
        }
        self
    }

    fn drain(&mut self, max_size: usize) -> std::collections::vec_deque::Drain<u64> {
        self.queue.drain(..self.queue.len().min(max_size))
    }
}

impl Default for AutoRefreshTimer {
    fn default() -> Self {
        let mut timer = Timer::from_seconds(30., true);
        timer.tick(
            timer
                .duration()
                .checked_sub(Duration::from_millis(100))
                .expect("there must be a pretty large refresh timer"),
        );
        Self {
            timer,
            queue: Default::default(),
        }
    }
}

fn periodly_refresh_all(
    time: Res<Time>,
    mut timer: ResMut<AutoRefreshTimer>,
    mut api_req_chan: EventWriter<ApiRequestEvent>,
    api_ctx: Res<api::Context>,
    cf: Res<AppConfig>,
) {
    if timer.timer.tick(time.delta()).just_finished() {
        info!("refresh a batch of userinfo");
        for uid in timer.refill(&cf).drain(cf.refresh_batch_size) {
            refresh_user_info(&mut api_req_chan, &api_ctx, uid);
        }
    }
}

fn refresh_user_info(
    api_req_chan: &mut EventWriter<ApiRequestEvent>,
    api_ctx: &Res<api::Context>,
    uid: u64,
) {
    info!("refresh userinfo of {}", uid);
    api_req_chan.send(ApiRequestEvent {
        req: api_ctx.new_user(uid).get_info(),
        tag: json!({"uid": uid, "cmd": "refresh"}).into(),
    });
    api_req_chan.send(ApiRequestEvent {
        req: api_ctx.new_user(uid).video_list(1),
        tag: json!({"uid": uid, "cmd": "new-video"}).into(),
    });
}

fn first_parse_api_result(
    mut raw_result: EventReader<ApiTaskResultEvent>,
    mut parsed: EventWriter<ParsedApiResult>,
    app_res: Res<AppResource>,
) {
    for ev in raw_result.iter() {
        let resp = match ev.result.as_ref() {
            Ok(r) => r,
            Err(e) => {
                error!("api error: {}", e);
                continue;
            }
        };
        let uid = match ev.tag["uid"].as_u64() {
            Some(u) => u,
            None => {
                debug!("result without uid: {:?}", ev);
                continue;
            }
        };
        let cmd = match ev.tag["cmd"].as_str() {
            Some(s) => s,
            None => {
                debug!("result without cmd: {:?}", ev);
                continue;
            }
        };
        match cmd {
            "refresh" => parsed.send(ParsedApiResult {
                uid,
                data: Data::Info(data::Info {
                    nickname: resp["name"].as_str().unwrap_or_default().to_string(),
                    live_room_url: resp["live_room"]["url"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    live_room_title: resp["live_room"]["title"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    live_open: resp["live_room"]["liveStatus"].as_i64().map(|x| x != 0),
                    live_entropy: resp["live_room"]["online"].as_u64().unwrap_or_default(),
                    face_url: resp["face"].as_str().unwrap_or_default().to_string(),
                }),
            }),
            "new-video" => {
                if let Some(list) = resp["list"]["vlist"].as_array() {
                    let v = list.iter().reduce(|a, b| {
                        let t1 = a["created"].as_i64().unwrap_or_default();
                        let t2 = b["created"].as_i64().unwrap_or_default();
                        if t1 > t2 {
                            a
                        } else {
                            b
                        }
                    });
                    parsed.send(ParsedApiResult {
                        uid,
                        data: Data::NewVideo(match v {
                            Some(vid) => data::NewVideo {
                                title: vid["title"].as_str().unwrap_or("N/A").to_string(),
                                date_time: NaiveDateTime::from_timestamp(
                                    vid["created"].as_i64().unwrap_or_default(),
                                    0,
                                )
                                .format("%Y-%m-%d %H:%M ")
                                .to_string(),
                                timestamp_sec: vid["created"].as_u64().unwrap_or_default(),
                            },
                            None => data::NewVideo {
                                title: app_res.no_video_text.clone(),
                                date_time: Default::default(),
                                timestamp_sec: Default::default(),
                            },
                        }),
                    });
                }
            }
            _ => error!("result with unimplemented cmd: {}", cmd),
        }
    }
}

fn sort_key_api_result(
    mut sort_key_query: Query<(&mut ui::following::data::SortKey, &ui::following::data::Uid)>,
    mut result_chan: EventReader<ParsedApiResult>,
) {
    for ParsedApiResult { uid, data } in result_chan.iter() {
        if let Data::Face(_) = data {
            continue;
        }
        if let Some((mut key, _)) = sort_key_query.iter_mut().find(|(_, id)| id.0 == *uid) {
            match data {
                Data::Info(info) => {
                    key.live_entropy = if let Some(true) = info.live_open {
                        info.live_entropy
                    } else {
                        0
                    }
                }
                Data::NewVideo(vid) => key.video_pub_ts = vid.timestamp_sec,
                _ => panic!("unimplement handler of {:?}", data),
            }
        }
    }
}
