use crate::*;
use bevy::utils::Duration;
use bilibili_api_rs::plugin::ApiRequestEvent;
use std::collections::VecDeque;

pub struct ModPlugin();

impl Plugin for ModPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.insert_resource(AutoRefreshTimer::default())
            .add_system(periodly_refresh_all.system());
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
            super::refresh_user_info(&mut api_req_chan, &api_ctx, uid);
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn check_queue_drain() {
        fn check_system(cf: Res<AppConfig>) {
            let mut a = AutoRefreshTimer::default();

            for _ in 0..5 {
                a.refill(&cf);
                assert_eq!(a.drain(30).cmp(0..30), Ordering::Equal);

                a.refill(&cf);
                assert_eq!(a.drain(30).cmp(30..60), Ordering::Equal);

                a.refill(&cf);
                assert_eq!(a.drain(30).cmp(60..90), Ordering::Equal);

                a.refill(&cf);
                assert_eq!(a.drain(30).cmp(90..100), Ordering::Equal);
            }
            assert_eq!(a.drain(30).count(), 0);
        }
        let mut cf = AppConfig::default();
        cf.followings_uid = (0..100).collect();

        let mut world = World::default();
        world.insert_resource(cf);

        let mut schedule = Schedule::default();
        let mut update = SystemStage::parallel();
        update.add_system(check_system.system());
        schedule.add_stage("update", update);
        schedule.run(&mut world);
    }
}
