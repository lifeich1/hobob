use crate::{db, Result};
use chrono::{DateTime, Local};
use rand::{seq::SliceRandom, Rng};
use serde_derive::{Deserialize, Serialize};
use std::convert::TryInto;
use std::fmt;
use std::io::{self, Write};
use std::sync::{Once, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

lazy_static::lazy_static! {
    static ref SENDER: RwLock<Option<mpsc::Sender<Command>>> = RwLock::new(None);

    static ref EVENTRX: RwLock<Option<watch::Receiver<Event>>> = RwLock::new(None);

    static ref SHUTDOWN: RwLock<Option<mpsc::Sender<i32>>> = RwLock::new(None);

    static ref SHUTDOWN_WAIT: RwLock<Option<mpsc::Receiver<i32>>> = RwLock::new(None);

    static ref ONCE: Once = Once::new();

    static ref SHUTDOWN_ONCE: Once = Once::new();
}

fn enforce_shutdown_struct_init() {
    SHUTDOWN_ONCE.call_once(|| {
        log::info!("init shutdown structures");
        let (tx, rx) = mpsc::channel(1);
        let mut sender = SHUTDOWN.write().expect("Write SHUTDOWN failure");
        let mut receiver = SHUTDOWN_WAIT.write().expect("Write SHUTDOWN_WAIT failure");
        *sender = Some(tx);
        *receiver = Some(rx);
    });
}

pub fn will_shutdown() -> mpsc::Sender<i32> {
    enforce_shutdown_struct_init();
    SHUTDOWN
        .read()
        .expect("Read lock SHUTDOWN failure")
        .as_ref()
        .expect("Initilizate SHUTDOWN failure or register too late")
        .clone()
}

pub async fn done_shutdown() {
    enforce_shutdown_struct_init();
    let tx = SHUTDOWN
        .write()
        .expect("Write lock SHUTDOWN failure")
        .take();
    drop(tx);
    let mut guard = SHUTDOWN_WAIT
        .write()
        .expect("Write lock SHUTDOWN_WAIT failure");
    let wait = guard
        .as_mut()
        .expect("Initilizate SHUTDOWN_WAIT failure")
        .recv();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    writeln!(&mut out, "\nDoing graceful shutdown ...").ok();
    writeln!(&mut out, "Press ^C again to halt").ok();
    tokio::select! {
        _ = wait => {},
        _ = tokio::signal::ctrl_c() => log::error!("forced halt"),
    };
}

pub const CHANNEL_CAP: usize = 128;
pub const SILENCE_HIP_TH: u64 = 5;

struct Engine {
    cmd: CommandRunner,
    refresh: RefreshRunner,
}

#[derive(Debug)]
pub enum Command {
    Refresh(i64),
    Follow(bool, i64),
    Activate,
    ForceSilence(bool),
    Shutdown,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum RefreshStatus {
    Fast,
    Slow,
    Silence(DateTime<Local>, String),
}

impl Default for RefreshStatus {
    fn default() -> Self {
        Self::Slow
    }
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Status(pub RefreshStatus);

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            RefreshStatus::Fast => write!(f, "激活自动刷新"),
            RefreshStatus::Slow => write!(f, "低速自动刷新"),
            RefreshStatus::Silence(i, _) => {
                let d = i - Local::now();
                let day = chrono::Duration::days(1);
                write!(
                    f,
                    "停止自动更新至{}",
                    i.format(if d > day {
                        "%Y-%m-%d %H:%M:%S"
                    } else {
                        "%H:%M:%S"
                    })
                )
            }
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Event {
    pub done_refresh: Option<i64>,
    pub status: Status,
    pub status_desc: String,
    pub new_live_msgid: i64,
    pub new_live_name: String,
}

fn enforce_init() {
    ONCE.call_once(|| {
        log::info!("Engine runners preparing ...");
        let (tx, rx) = mpsc::channel(CHANNEL_CAP);
        let (etx, erx) = watch::channel(Event::default());
        tokio::spawn(async move {
            let engine = Engine::new(rx, etx);
            engine.run().await;
        });
        let mut sender = SENDER.write().expect("Write lock SENDER failure");
        *sender = Some(tx);
        let mut evrx = EVENTRX.write().expect("Write lock EVENTRX failure");
        *evrx = Some(erx);
    });
}

pub fn event_rx() -> watch::Receiver<Event> {
    enforce_init();
    EVENTRX
        .read()
        .expect("Read lock EVENTRX failure")
        .as_ref()
        .expect("Initilizate EVENTRX failure")
        .clone()
}

pub fn handle() -> mpsc::Sender<Command> {
    enforce_init();
    SENDER
        .read()
        .expect("Read lock SENDER failure")
        .as_ref()
        .expect("Initilizate SENDER failure")
        .clone()
}

struct CommandRunner {
    receiver: mpsc::Receiver<Command>,
    dispatcher: CommandDispatcher,
}

#[derive(Clone)]
struct CommandDispatcher {
    refresh_sender: mpsc::Sender<Command>,
}

impl CommandRunner {
    pub async fn run(mut self) {
        log::info!("CommandRunner started");
        let _running = will_shutdown();
        while let Some(cmd) = self.receiver.recv().await {
            let mut d = self.dispatcher.clone();
            let quit = matches!(cmd, Command::Shutdown);
            tokio::spawn(async move {
                d.dispatch(cmd).await;
            });
            if quit {
                break;
            }
        }
        log::info!("CommandRunner stopped");
    }
}

impl CommandDispatcher {
    pub async fn dispatch(&mut self, cmd: Command) {
        log::trace!("Command refresh type");
        self.refresh_sender.send(cmd).await.ok();
    }
}

struct RefreshRunner {
    receiver: mpsc::Receiver<Command>,
    token: RefreshBucket,
    api: bilibili_api_rs::Client,
    evtx: watch::Sender<Event>,
    silence_cnt: u64,
    silence_reason: String,
}

impl RefreshRunner {
    pub fn new(receiver: mpsc::Receiver<Command>, evtx: watch::Sender<Event>) -> Self {
        Self {
            receiver,
            evtx,
            token: Default::default(),
            api: bilibili_api_rs::Client::new(),
            silence_cnt: 0,
            silence_reason: Default::default(),
        }
    }

    pub async fn run(mut self) {
        log::info!("RefreshRunner started");
        let _running = will_shutdown();
        let slowdown_duration = 32 * REFRESH_BUCKET_TIK_INTERVAL;
        let auto_refresh = tokio::time::sleep(REFRESH_BUCKET_TIK_INTERVAL);
        let auto_slowdown = tokio::time::sleep(slowdown_duration);
        tokio::pin!(auto_refresh);
        tokio::pin!(auto_slowdown);
        let factors: Vec<f32> = {
            let mut rng = rand::thread_rng();
            (0..100).map(|_| rng.gen_range(1.0..2.0)).collect()
        };
        let mut factor_i: usize = 0;
        let live_pagens: Vec<i32> = {
            let mut rng = rand::thread_rng();
            let mut v: Vec<i32> = (1..21).collect();
            v.shuffle(&mut rng);
            v.into_iter().flat_map(|x| [0, x]).collect()
        };
        log::info!("init pseudo random live_pagens: {:?}", live_pagens);
        let mut live_pagens_i: usize = 0;

        loop {
            tokio::select! {
                cmd = self.receiver.recv() => {
                    if matches!(cmd, None) {
                        log::error!("Refresh command dispatch channel closed");
                        break;
                    }
                    log::debug!("Trigger active speed token bucket");
                    auto_slowdown.as_mut().reset(tokio::time::Instant::now() + slowdown_duration);
                    self.status_change(RefreshStatus::Fast);
                    self.token.set_interval(REFRESH_BUCKET_TIK_INTERVAL);
                    match cmd.unwrap() {
                        Command::Refresh(uid) => self.try_refresh(db::User::new(uid), 0).await,
                        Command::Follow(enable, uid) => {
                            let u = db::User::new(uid);
                            u.enable(enable);
                            if enable {
                                self.try_refresh(u, 0).await;
                            }
                        },
                        Command::Activate => log::info!("Command Activate force token bucket high speed"),
                        Command::ForceSilence(flag) => {
                            log::info!("Command ForceSilence {}", flag);
                            if flag {
                                self.on_remote_api_err("Forced");
                            } else {
                                self.on_remote_api_ok();
                                self.token.available_now();
                            }
                            self.status_change(RefreshStatus::Fast);
                        },
                        Command::Shutdown => break,
                    }
                }
                _ = &mut auto_refresh => {
                    let factor: f32 = factors[factor_i];
                    factor_i += 1;
                    if factor_i >= factors.len() {
                        factor_i = 0;
                    }
                    let pn: i32 = live_pagens[live_pagens_i];
                    live_pagens_i += 1;
                    if live_pagens_i >= live_pagens.len() {
                        live_pagens_i = 0;
                    }
                    auto_refresh.as_mut().reset(tokio::time::Instant::now() + REFRESH_BUCKET_TIK_INTERVAL.mul_f32(factor));
                    match db::User::oldest_ctime_user() {
                        Ok(user) => self.try_refresh(user, pn).await,
                        Err(e) => log::error!("Database query oldest ctime user error(s): {}", e),
                    }
                }
                _ = &mut auto_slowdown => {
                    auto_slowdown.as_mut().reset(tokio::time::Instant::now() + REFRESH_BUCKET_TIK_INTERVAL * 3600);
                    log::warn!("Trigger slowing down token bucket");
                    self.status_change(RefreshStatus::Slow);
                    self.token.set_interval(REFRESH_BUCKET_TIK_INTERVAL * 10);
                }
            }
        }
        log::info!("RefreshRunner stopped");
    }

    async fn try_refresh(&mut self, user: db::User, live_pn: i32) {
        if !self.token.try_once() {
            if self.token.is_need_log() {
                log::info!(
                    "Canceled refresh uid {} live_pn {} for no token",
                    user.id(),
                    live_pn
                );
            }
            return;
        }
        if live_pn == 0 {
            let id = user.id();
            match self.refresh(user).await {
                Ok(_) => self.on_remote_api_ok(),
                Err(e) => {
                    self.on_remote_api_err(&e);
                    log::error!("Refresh uid {} error(s): {}", id, e);
                }
            }
        } else {
            match self.refresh_live_list(live_pn).await {
                Ok(_) => self.on_remote_api_ok(),
                Err(e) => {
                    self.on_remote_api_err(&e);
                    log::error!("Request vtb live page {} error(s): {}", live_pn, e);
                }
            }
        }
    }

    async fn refresh(&mut self, user: db::User) -> Result<()> {
        let last_info = user.info();

        let api = self.api.user(user.id());
        let info: db::UserInfo = api.info().await?.try_into()?;
        user.set_info(&info);
        let videos: db::VideoVector = api.latest_videos().await?.try_into()?;
        user.update_videos(videos.iter());

        let uid = user.id();
        log::info!("Refresh ok uid {}", uid);
        self.event_change(|ev| {
            ev.done_refresh = Some(uid);
            if matches!(ev.status.0, RefreshStatus::Silence(_, _)) {
                ev.status.0 = RefreshStatus::Slow;
                ev.status_desc = ev.status.to_string();
            }
        });
        if !matches!(
            last_info,
            Ok(db::UserInfo {
                live_open: Some(true),
                ..
            })
        ) && matches!(info.live_open, Some(true))
        {
            self.on_new_live(&info);
        }

        Ok(())
    }

    async fn refresh_live_list(&self, live_pn: i32) -> Result<()> {
        if live_pn <= 0 {
            return Ok(());
        }

        // Vup/all
        let xlive = self.api.xlive(9, 0);
        let v = xlive.list(live_pn.into()).await?;
        let l = match v["list"].as_array() {
            Some(l) => l,
            None => {
                log::error!("xlive list result['list'] is not list: {:?}", v);
                return Ok(());
            }
        };
        log::info!("xlive get page {}", live_pn);
        for i in l.iter() {
            if let Err(e) = self.try_parse_livelist_row(i) {
                log::error!("try parse livelist row error: {}", e);
            }
        }
        Ok(())
    }

    fn try_parse_livelist_row(&self, i: &serde_json::Value) -> Result<()> {
        let uid = match i["uid"].as_i64() {
            Some(v) => v,
            None => {
                log::error!("xlive list row uid not num: {:?}", i);
                return Ok(());
            }
        };
        let u = db::User::new(uid);
        let mut info = match u.info() {
            Ok(v) => v,
            Err(e) => {
                log::trace!("uid {} not in db: {}", uid, e);
                return Ok(());
            }
        };
        if !matches!(info.live_open, Some(true)) {
            self.on_new_live(&info);
        }
        info.live_open = Some(true);
        if let Some(link) = i["link"].as_str() {
            info.live_room_url = Some(format!("https://live.bilibili.com{}", link));
        }
        if let Some(title) = i["title"].as_str() {
            info.live_room_title = Some(title.to_string());
        }
        if let Some(online) = i["watched_show"]["num"].as_i64() {
            info.live_entropy = Some(online);
        }
        log::trace!("update live info from livelist row for {}", uid);
        u.set_info(&info);
        Ok(())
    }

    fn event_change<F: FnMut(&mut Event)>(&self, mut f: F) {
        let mut ev = self.evtx.borrow().clone();
        f(&mut ev);
        self.evtx.send(ev).ok();
    }

    fn status_change(&self, stat: RefreshStatus) {
        let s = if self.silence_cnt >= SILENCE_HIP_TH {
            RefreshStatus::Silence(
                to_datetime(self.token.next_tik()),
                self.silence_reason.clone(),
            )
        } else {
            stat
        };
        self.event_change(move |ev| {
            ev.status.0 = s.clone();
            ev.status_desc = ev.status.to_string();
        });
    }

    fn on_new_live(&self, info: &db::UserInfo) {
        log::info!("uid {} live open: {}", info.id, info.name);
        let n = info.name.to_string();
        self.event_change(move |ev| {
            ev.new_live_msgid += 1;
            ev.new_live_name = n.clone();
        });
    }

    fn on_remote_api_err<T: ToString>(&mut self, reason: T) {
        self.silence_cnt += 1;
        let why = reason.to_string();
        log::error!(
            "increase silence count {}/{}, reason: {}",
            self.silence_cnt,
            SILENCE_HIP_TH,
            &why
        );
        if self.silence_cnt >= SILENCE_HIP_TH {
            self.silence_reason = why;
            self.token
                .silence(Duration::from_secs(60 * self.silence_cnt));
        }
    }

    fn on_remote_api_ok(&mut self) {
        self.silence_cnt = 0;
    }
}

fn to_datetime(i: Instant) -> DateTime<Local> {
    let now = Instant::now();
    if let Some(d) = i.checked_duration_since(now) {
        Local::now()
            + chrono::Duration::from_std(d).unwrap_or_else(|e| {
                log::error!("chrono::Duration::from_std error(s): {}", e);
                chrono::Duration::zero()
            })
    } else if let Some(d) = now.checked_duration_since(i) {
        Local::now()
            - chrono::Duration::from_std(d).unwrap_or_else(|e| {
                log::error!("chrono::Duration::from_std error(s): {}", e);
                chrono::Duration::zero()
            })
    } else {
        log::error!("instant {:?} cast to datetime failed", i);
        Local::now()
    }
}

pub const REFRESH_BUCKET_CAP: i32 = 30;
pub const REFRESH_BUCKET_TIK_INTERVAL: Duration = Duration::from_secs(5);

struct RefreshBucket {
    interval: Duration,
    tik: Instant,
    now: i32,
    canceled: i32,
}

impl Default for RefreshBucket {
    fn default() -> Self {
        Self {
            interval: REFRESH_BUCKET_TIK_INTERVAL,
            tik: Instant::now(),
            now: REFRESH_BUCKET_CAP,
            canceled: 0,
        }
    }
}

impl Engine {
    pub fn new(receiver: mpsc::Receiver<Command>, evtx: watch::Sender<Event>) -> Self {
        let (tx0, rx0) = mpsc::channel(CHANNEL_CAP);
        Self {
            cmd: CommandRunner {
                receiver,
                dispatcher: CommandDispatcher {
                    refresh_sender: tx0,
                },
            },
            refresh: RefreshRunner::new(rx0, evtx),
        }
    }

    pub async fn run(self) {
        let Self { cmd, refresh } = self;
        tokio::spawn(async move {
            cmd.run().await;
        });
        tokio::spawn(async move {
            refresh.run().await;
        });
    }
}

impl RefreshBucket {
    pub fn try_once(&mut self) -> bool {
        let now = Instant::now();
        let d: i32 = (now
            .checked_duration_since(self.tik)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            / self.interval.as_secs().max(1u64)) as i32;
        if d > 0 {
            self.now += d;
            self.tik += self.interval * d as u32;
        }
        if self.now > 0 {
            self.now = self.now.min(REFRESH_BUCKET_CAP) - 1;
            self.canceled = 0;
            true
        } else {
            if self.canceled < 2 {
                self.canceled += 1
            }
            false
        }
    }

    pub fn is_need_log(&self) -> bool {
        self.canceled < 2
    }

    pub fn set_interval(&mut self, i: Duration) {
        self.interval = i;
    }

    pub fn silence(&mut self, d: Duration) {
        self.tik += d;
        log::info!(
            "silence token bucket {} secs, original remaining about {} token(s)",
            d.as_secs(),
            self.now
        );
        self.now = 0;
    }

    pub fn available_now(&mut self) {
        let now = Instant::now();
        if self.tik > now {
            self.tik = now;
        }
    }

    pub fn next_tik(&self) -> Instant {
        self.tik
    }
}
