use crate::*;
use bilibili_api_rs::plugin::ApiRequestEvent;
use hobob_bevy_widget::scroll;
use serde_json::json;

mod face;
mod parser;
mod timer;

pub struct ModPlugin();

impl Plugin for ModPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.add_system(handle_actions.system())
            .add_plugin(face::ModPlugin())
            .add_plugin(timer::ModPlugin())
            .add_plugin(parser::ModPlugin());
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_actions(
    mut action_chan: EventReader<ui::following::event::Action>,
    mut api_req_chan: EventWriter<ApiRequestEvent>,
    api_ctx: Res<api::Context>,
    visible_nickname_query: Query<(&ui::following::Nickname, &Visible)>,
    mut cf: ResMut<AppConfig>,
    app_res: Res<AppResource>,
    mut commands: Commands,
    mut scroll_widget_query: Query<(Entity, &mut scroll::ScrollSimListWidget)>,
) {
    for action in action_chan.iter() {
        match action {
            ui::following::event::Action::RefreshVisible => {
                refresh_visible(&mut api_req_chan, &api_ctx, &visible_nickname_query)
            }
            ui::following::event::Action::AddFollowingUid(uid) => add_following(
                *uid,
                &mut cf,
                &app_res,
                &mut commands,
                &mut scroll_widget_query,
                &mut api_req_chan,
                &api_ctx,
            ),
        }
    }
}

fn refresh_visible(
    api_req_chan: &mut EventWriter<ApiRequestEvent>,
    api_ctx: &Res<api::Context>,
    visible_nickname_query: &Query<(&ui::following::Nickname, &Visible)>,
) {
    for (nickname, _) in visible_nickname_query
        .iter()
        .filter(|(_, visible)| visible.is_visible)
    {
        refresh_user_info(api_req_chan, api_ctx, nickname.0);
    }
}

fn add_following(
    uid: u64,
    cf: &mut ResMut<AppConfig>,
    app_res: &Res<AppResource>,
    commands: &mut Commands,
    scroll_widget_query: &mut Query<(Entity, &mut scroll::ScrollSimListWidget)>,
    api_req_chan: &mut EventWriter<ApiRequestEvent>,
    api_ctx: &Res<api::Context>,
) {
    if !cf.add_following(uid) {
        info!("already following {}", uid);
        return;
    }
    for (entity, mut scroll_widget) in scroll_widget_query.iter_mut() {
        let widget = widget::create_following(commands, app_res, uid);
        commands.entity(entity).insert_children(0, &[widget]);
        scroll_widget.scroll_to_top();
        refresh_user_info(api_req_chan, api_ctx, uid);
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
