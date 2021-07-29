use crate::*;
use bevy::{
    app::AppExit,
    input::{
        keyboard::{KeyCode, KeyboardInput},
        mouse::{MouseScrollUnit, MouseWheel},
        ElementState,
    },
};
use clipboard::{ClipboardContext, ClipboardProvider};
use hobob_bevy_widget::scroll;

pub struct ModPlugin();

impl Plugin for ModPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.add_system(input.system())
            .add_system(jump_button_system.system())
            .add_system(button_add_following.system())
            .add_system(on_filter_button.system())
            .add_system(button_refresh.system());
    }
}

fn try_get_pasted() -> Result<String, Box<dyn std::error::Error>> {
    ClipboardContext::new()?.get_contents()
}

fn input(
    mut keyboard_ev: EventReader<KeyboardInput>,
    mut mousewheel: EventReader<MouseWheel>,
    mut exit_ev: EventWriter<AppExit>,
    keyboard: Res<Input<KeyCode>>,
    mut scroll_widget_query: Query<&mut scroll::ScrollSimListWidget>,
    mut adding_following_query: Query<&mut Text, With<ui::add::AddFollowing>>,
) {
    let mut scroll_move: i32 = 0;
    let mut text_edit = Vec::<KeyCode>::new();
    for ev in keyboard_ev.iter() {
        match ev {
            KeyboardInput {
                key_code: Some(KeyCode::Escape),
                state: ElementState::Released,
                ..
            } => {
                info!("key ESC released");
                exit_ev.send(AppExit {});
            }
            KeyboardInput {
                key_code: Some(k @ (KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right)),
                state: ElementState::Released,
                ..
            } => {
                scroll_move = match k {
                    KeyCode::Up => -1,
                    KeyCode::Down => 1,
                    KeyCode::Left => -4,
                    KeyCode::Right => 4,
                    _ => panic!("match scroll_move at unexpected key: {:?}", k),
                };
            }
            KeyboardInput {
                key_code:
                    Some(
                        k
                        @
                        (KeyCode::Key0
                        | KeyCode::Key1
                        | KeyCode::Key2
                        | KeyCode::Key3
                        | KeyCode::Key4
                        | KeyCode::Key5
                        | KeyCode::Key6
                        | KeyCode::Key7
                        | KeyCode::Key8
                        | KeyCode::Key9
                        | KeyCode::Back
                        | KeyCode::Paste),
                    ),
                state: ElementState::Pressed,
                ..
            } => {
                text_edit.push(*k);
            }
            _ => (),
        }
    }

    if keyboard.pressed(KeyCode::LControl) && keyboard.just_pressed(KeyCode::V) {
        text_edit.push(KeyCode::Paste);
    }

    if scroll_move == 0 {
        for ev in mousewheel.iter() {
            if let MouseWheel {
                unit: MouseScrollUnit::Line,
                x: _,
                y,
            } = ev
            {
                if y.abs() > f32::EPSILON {
                    scroll_move -= (y.abs().ceil() * y.signum()) as i32;
                }
            }
        }
    }

    if scroll_move != 0 {
        for mut widget in scroll_widget_query.iter_mut() {
            widget.scroll_to(scroll_move);
        }
    }

    if !text_edit.is_empty() {
        for mut text in adding_following_query.iter_mut() {
            let v = &mut text.sections[0].value;
            for k in text_edit.iter() {
                match k {
                    KeyCode::Key0 => v.push('0'),
                    KeyCode::Key1 => v.push('1'),
                    KeyCode::Key2 => v.push('2'),
                    KeyCode::Key3 => v.push('3'),
                    KeyCode::Key4 => v.push('4'),
                    KeyCode::Key5 => v.push('5'),
                    KeyCode::Key6 => v.push('6'),
                    KeyCode::Key7 => v.push('7'),
                    KeyCode::Key8 => v.push('8'),
                    KeyCode::Key9 => v.push('9'),
                    KeyCode::Back => {
                        v.pop();
                    }
                    KeyCode::Paste => match try_get_pasted() {
                        Ok(s) => v.push_str(s.as_str()),
                        Err(e) => error!("get content from clipboard error: {}", e),
                    },
                    _ => panic!("match text edit op at unexpected key: {:?}", k),
                }
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn jump_button_system(
    app_res: Res<AppResource>,
    button_query: Query<
        (
            &Interaction,
            &ui::following::HoverPressShow,
            Option<&ui::following::HomepageOpenButton>,
            Option<&ui::following::LiveRoomOpenButton>,
        ),
        (Changed<Interaction>, With<Button>),
    >,
    mut shower_query: Query<(&ui::following::HoverPressShower, &mut Handle<ColorMaterial>)>,
) {
    for (interaction, show, opt_home, opt_live) in button_query.iter() {
        let (uid, url) = match (opt_home, opt_live) {
            (Some(home), None) => (home.0, format!("https://space.bilibili.com/{}/", home.0)),
            (None, Some(live)) => (live.0, live.1.to_string()),
            _ => panic!(
                "HoverPressShow widget invalid status: {:?} {:?}",
                opt_home, opt_live
            ),
        };
        if url.is_empty() {
            continue;
        }
        let entity = show.0;
        let shower = shower_query
            .get_component::<ui::following::HoverPressShower>(entity)
            .expect("entity in shower_query must have component HoverPressShower");
        if shower.0 != uid {
            panic!("HoverPressShow(er) uid mismatch: {} {}", shower.0, uid);
        }

        match interaction {
            Interaction::Clicked => {
                let open_cmd = if cfg!(target_os = "linux") {
                    "xdg-open"
                } else {
                    "open"
                };
                let start = std::process::Command::new(open_cmd).arg(&url).spawn();
                match start {
                    Ok(_) => info!("open url ok: {}", url),
                    Err(e) => error!("open url error: {}", e),
                }
            }
            Interaction::Hovered | Interaction::None => {
                let mut material = shower_query
                    .get_component_mut::<Handle<ColorMaterial>>(entity)
                    .expect("entity in shower_query must have component Handle<ColorMaterial>");
                *material = if let Interaction::None = interaction {
                    app_res.item_bg_col.clone()
                } else {
                    app_res.item_to_jump_bg_col.clone()
                };
            }
        }
    }
}

fn on_filter_button(
    app_res: Res<AppResource>,
    mut children_query: Query<(&mut Children, &mut scroll::ScrollSimListWidget)>,
    key_query: Query<&ui::following::data::SortKey>,
    mut interaction_query: Query<(
        &Interaction,
        &mut Handle<ColorMaterial>,
        &ui::filter::ReorderButton,
    )>,
) {
    for (interaction, mut material, reorder_type) in interaction_query.iter_mut() {
        match *interaction {
            Interaction::Hovered => {
                *material = app_res.btn_hover_col.clone();
            }
            Interaction::None => {
                *material = app_res.btn_none_col.clone();
            }
            Interaction::Clicked => {
                *material = app_res.btn_press_col.clone();
                for (mut children, mut widget) in children_query.iter_mut() {
                    let mut idx: Vec<(usize, u64)> = match reorder_type.0 {
                        ui::filter::Filter::LiveEntropy => children
                            .iter()
                            .map(|entity| {
                                key_query
                                    .get_component::<ui::following::data::SortKey>(*entity)
                                    .unwrap()
                                    .live_entropy
                            })
                            .enumerate()
                            .collect(),
                        ui::filter::Filter::VideoPub => children
                            .iter()
                            .map(|entity| {
                                key_query
                                    .get_component::<ui::following::data::SortKey>(*entity)
                                    .unwrap()
                                    .video_pub_ts
                            })
                            .enumerate()
                            .collect(),
                    };
                    idx.sort_by(|a, b| a.1.cmp(&b.1).reverse());
                    let mut swap_from: Vec<usize> = idx.iter().map(|x| x.0).collect();
                    let mut swap_to = Vec::<usize>::new();
                    swap_to.resize(swap_from.len(), 0);
                    for (i, x) in swap_from.iter().enumerate() {
                        swap_to[*x] = i;
                    }
                    for i in 0..children.len() {
                        if i < swap_from[i] {
                            children.swap(i, swap_from[i]);
                            if swap_to[i] > i {
                                swap_from[swap_to[i]] = swap_from[i];
                                swap_to[swap_from[i]] = swap_to[i];
                            }
                        }
                    }
                    widget.scroll_to_top();
                }
            }
        }
    }
}
#[allow(clippy::type_complexity)]
fn button_add_following(
    app_res: Res<AppResource>,
    mut interaction_query: Query<
        (&Interaction, &mut Handle<ColorMaterial>),
        (
            With<Button>,
            Changed<Interaction>,
            With<ui::add::AddFollowingButton>,
        ),
    >,
    add_query: Query<&Text, With<ui::add::AddFollowing>>,
    mut action_chan: EventWriter<ui::following::event::Action>,
) {
    for (interaction, mut material) in interaction_query.iter_mut() {
        match *interaction {
            Interaction::Clicked => {
                let mut uid: Option<u64> = None;
                for add in add_query.iter() {
                    if !add.sections.is_empty() {
                        uid = add.sections[0].value.parse::<u64>().ok();
                        if uid.is_some() {
                            break;
                        }
                    }
                }
                match uid {
                    Some(id) => {
                        info!("button add following trigger: {}", id);
                        action_chan.send(ui::following::event::Action::AddFollowingUid(id));
                    }
                    None => info!("parse input error: button add following"),
                }
                *material = app_res.btn_press_col.clone();
            }
            Interaction::Hovered => {
                *material = app_res.btn_hover_col.clone();
            }
            Interaction::None => {
                *material = app_res.btn_none_col.clone();
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn button_refresh(
    interaction_query: Query<
        &Interaction,
        (
            With<Button>,
            Changed<Interaction>,
            With<ui::add::RefreshVisible>,
        ),
    >,
    mut action_chan: EventWriter<ui::following::event::Action>,
) {
    if let Some(_) = interaction_query.iter().find(|i| matches!(*i, Interaction::Clicked)) {
        info!("button refresh trigger!");
        action_chan.send(ui::following::event::Action::RefreshVisible);
    }
}
