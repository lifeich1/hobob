use crate::{db, Result};
use std::convert::TryInto;
use std::sync::{Once, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

lazy_static::lazy_static! {
    static ref SENDER: RwLock<Option<mpsc::Sender<Command>>> = RwLock::new(None);

    static ref EVENTRX: RwLock<Option<watch::Receiver<Event>>> = RwLock::new(None);

    static ref ONCE: Once = Once::new();
}

pub const CHANNEL_CAP: usize = 128;

struct Engine {
    cmd: CommandRunner,
    refresh: RefreshRunner,
}

pub enum Command {
    Refresh(i64),
    Follow(bool, i64),
}

#[derive(Clone, Default)]
pub struct Event {
    done_refresh: Option<i64>,
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
        while let Some(cmd) = self.receiver.recv().await {
            let mut d = self.dispatcher.clone();
            tokio::spawn(async move {
                d.dispatch(cmd).await;
            });
        }
        log::error!("CommandRunner stopped");
    }
}

impl CommandDispatcher {
    pub async fn dispatch(&mut self, cmd: Command) {
        match cmd {
            Command::Refresh(_) | Command::Follow(_, _) => {
                log::trace!("Command refresh type");
                self.refresh_sender.send(cmd).await.ok();
            }
        }
    }
}

struct RefreshRunner {
    receiver: mpsc::Receiver<Command>,
    token: RefreshBucket,
    ctx: bilibili_api_rs::Context,
    evtx: watch::Sender<Event>,
}

impl RefreshRunner {
    pub fn new(receiver: mpsc::Receiver<Command>, evtx: watch::Sender<Event>) -> Self {
        Self {
            receiver,
            evtx,
            token: Default::default(),
            ctx: bilibili_api_rs::Context::new()
                .unwrap_or_else(|e| panic!("New bilibili api context error(s): {}", e)),
        }
    }

    pub async fn run(mut self) {
        log::info!("RefreshRunner started");
        let auto_refresh = tokio::time::sleep(REFRESH_BUCKET_TIK_INTERVAL);
        tokio::pin!(auto_refresh);

        loop {
            tokio::select! {
                cmd = self.receiver.recv() => {
                    if matches!(cmd, None) {
                        log::error!("Refresh command dispatch channel closed");
                        break;
                    }
                    match cmd.unwrap() {
                        Command::Refresh(uid) => self.try_refresh(db::User::new(uid)).await,
                        Command::Follow(enable, uid) => {
                            let u = db::User::new(uid);
                            u.enable(enable);
                            if enable {
                                self.try_refresh(u).await;
                            }
                        },
                    }
                }
                _ = &mut auto_refresh => {
                    auto_refresh.as_mut().reset(tokio::time::Instant::now() + REFRESH_BUCKET_TIK_INTERVAL);
                    match db::User::oldest_ctime_user() {
                        Ok(user) => self.try_refresh(user).await,
                        Err(e) => log::error!("Database query oldest ctime user error(s): {}", e),
                    }
                }
            }
        }
        log::error!("RefreshRunner stopped");
    }

    async fn try_refresh(&mut self, user: db::User) {
        if !self.token.try_once() {
            log::info!("Canceled refresh uid {} for no token", user.id());
            return;
        }
        let id = user.id();
        self.refresh(user)
            .await
            .map_err(|e| log::error!("Refresh uid {} error(s): {}", id, e))
            .ok();
    }

    async fn refresh(&mut self, user: db::User) -> Result<()> {
        let api = self.ctx.new_user(&user);
        let info: db::UserInfo = api.get_info()?.invalidate().query().await?.try_into()?;
        user.set_info(&info);
        let videos: db::VideoVector = api.video_list(1)?.invalidate().query().await?.try_into()?;
        user.update_videos(videos.iter());

        let uid = user.id();
        log::info!("Refresh ok uid {}", uid);
        self.event_change(|ev| ev.done_refresh = Some(uid));

        Ok(())
    }

    fn event_change<F: FnMut(&mut Event)>(&self, mut f: F) {
        let mut ev = self.evtx.borrow().clone();
        f(&mut ev);
        self.evtx.send(ev).ok();
    }
}

pub const REFRESH_BUCKET_CAP: i32 = 30;
pub const REFRESH_BUCKET_TIK_INTERVAL: Duration = Duration::from_secs(5);

struct RefreshBucket {
    tik: Instant,
    now: i32,
}

impl Default for RefreshBucket {
    fn default() -> Self {
        Self {
            tik: Instant::now(),
            now: REFRESH_BUCKET_CAP,
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
        let d: i32 =
            ((now - self.tik).as_secs() / REFRESH_BUCKET_TIK_INTERVAL.as_secs().max(1u64)) as i32;
        if d > 0 {
            self.now += d;
            self.tik += REFRESH_BUCKET_TIK_INTERVAL * d as u32;
        }
        if self.now > 0 {
            self.now = self.now.min(REFRESH_BUCKET_CAP) - 1;
            true
        } else {
            false
        }
    }
}
