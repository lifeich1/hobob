use crate::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use bilibili_api_rs::plugin::{ApiRequestTag, ApiTaskResultEvent};
use chrono::naive::NaiveDateTime;
use futures_lite::future;
use ui::following::{
    data::{self, Data},
    event::ParsedApiResult,
};

pub struct ModPlugin();

impl Plugin for ModPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.add_system(watch_and_parse_api_result.system())
            .add_system(sort_key_api_result.system());
    }
}

fn parse_api_result(resp: serde_json::Value, tag: ApiRequestTag) -> Option<ParsedApiResult> {
    let uid = match tag["uid"].as_u64() {
        Some(u) => u,
        None => {
            debug!("result without uid: {:?} {:?}", resp, tag);
            return None;
        }
    };
    let cmd = match tag["cmd"].as_str() {
        Some(s) => s,
        None => {
            debug!("result without cmd: {:?} {:?}", resp, tag);
            return None;
        }
    };
    debug!("api result: cmd {} uid {}", cmd, uid);
    match cmd {
        "refresh" => Some(ParsedApiResult {
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
                Some(ParsedApiResult {
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
                        None => data::NewVideo::default(),
                    }),
                })
            } else {
                None
            }
        }
        _ => {
            error!("result with unimplemented cmd: {}", cmd);
            None
        }
    }
}

struct AsyncParseResult(Option<ParsedApiResult>);

fn watch_and_parse_api_result(
    mut raw_result: EventReader<ApiTaskResultEvent>,
    mut parsed: EventWriter<ParsedApiResult>,
    thread_pool: Res<AsyncComputeTaskPool>,
    mut commands: Commands,
    mut tasks: Query<(Entity, &mut Task<AsyncParseResult>)>,
) {
    for ev in raw_result.iter() {
        let resp = match ev.result.as_ref() {
            Ok(r) => r.clone(),
            Err(e) => {
                error!("api error: {}", e);
                continue;
            }
        };
        let tag = ev.tag.clone();
        let task = thread_pool.spawn(async move { AsyncParseResult(parse_api_result(resp, tag)) });
        commands.spawn().insert(task);
    }

    for (entity, mut task) in tasks.iter_mut() {
        if let Some(task_result) = future::block_on(future::poll_once(&mut *task)) {
            if let Some(result) = task_result.0 {
                parsed.send(result);
            }
            commands.entity(entity).remove::<Task<AsyncParseResult>>();
            commands.entity(entity).despawn();
        }
    }
}

fn sort_key_api_result(
    mut sort_key_query: Query<(&mut ui::following::data::SortKey, &ui::following::data::Uid)>,
    mut result_chan: EventReader<ParsedApiResult>,
) {
    for ParsedApiResult { uid, data } in result_chan
        .iter()
        .filter(|ParsedApiResult { data, .. }| matches!(data, Data::Info(_) | Data::NewVideo(_)))
    {
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
