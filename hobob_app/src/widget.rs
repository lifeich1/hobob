use super::*;

pub fn create_following(commands: &mut Commands, app_res: &Res<AppResource>, uid: u64) -> Entity {
    let span = commands
        .spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Percent(100.), Val::Px(8.)),
                ..Default::default()
            },
            material: app_res.none_col.clone(),
            ..Default::default()
        })
        .id();
    let item_layout = commands
        .spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Auto, Val::Percent(20.)),
                flex_grow: 100.0,
                flex_direction: FlexDirection::Row,
                ..Default::default()
            },
            material: app_res.item_bg_col.clone(),
            ..Default::default()
        })
        .id();
    let face = commands
        .spawn_bundle(ImageBundle {
            style: Style {
                size: Size::new(Val::Auto, Val::Percent(100.)),
                aspect_ratio: Some(1.),
                ..Default::default()
            },
            material: app_res.face_none_img.clone(),
            ..Default::default()
        })
        .insert(ui::following::Face(uid))
        .id();
    let description_layout = commands
        .spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Percent(50.), Val::Percent(100.)),
                border: Rect::all(Val::Px(4.)),
                flex_direction: FlexDirection::ColumnReverse,
                flex_grow: 100.,
                ..Default::default()
            },
            material: app_res.none_col.clone(),
            ..Default::default()
        })
        .id();
    let nickname = commands
        .spawn_bundle(TextBundle {
            style: Style {
                size: Size::new(Val::Percent(100.0), Val::Percent(100.0)),
                ..Default::default()
            },
            text: Text::with_section(
                format!("#{}", uid),
                TextStyle {
                    font: app_res.font.clone(),
                    font_size: 20.,
                    color: Color::BLUE,
                },
                TextAlignment {
                    horizontal: HorizontalAlign::Center,
                    ..Default::default()
                },
            ),
            ..Default::default()
        })
        .insert(ui::following::Nickname(uid))
        .id();
    let livetitle = commands
        .spawn_bundle(TextBundle {
            style: Style {
                size: Size::new(Val::Percent(100.0), Val::Percent(100.0)),
                ..Default::default()
            },
            text: Text::with_section(
                "[loading live info...]",
                TextStyle {
                    font: app_res.font.clone(),
                    font_size: 15.,
                    color: Color::BLACK,
                },
                TextAlignment {
                    horizontal: HorizontalAlign::Center,
                    ..Default::default()
                },
            ),
            ..Default::default()
        })
        .insert(ui::following::LiveRoomTitle(uid))
        .id();
    let videoinfo = commands
        .spawn_bundle(TextBundle {
            style: Style {
                size: Size::new(Val::Auto, Val::Percent(30.0)),
                ..Default::default()
            },
            text: Text::with_section(
                "[loading video info...]",
                TextStyle {
                    font: app_res.font.clone(),
                    font_size: 15.,
                    color: Color::BLACK,
                },
                TextAlignment {
                    horizontal: HorizontalAlign::Center,
                    ..Default::default()
                },
            ),
            ..Default::default()
        })
        .insert(ui::following::VideoInfo(uid))
        .id();
    let homepage = commands
        .spawn_bundle(ButtonBundle {
            style: Style {
                size: Size::new(Val::Auto, Val::Percent(100.0)),
                flex_grow: 0.0,
                ..Default::default()
            },
            material: app_res.none_col.clone(),
            ..Default::default()
        })
        .insert(ui::following::HomepageOpenButton(uid))
        .id();
    let homepage_layout = commands
        .spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Auto, Val::Percent(30.0)),
                flex_direction: FlexDirection::Row,
                ..Default::default()
            },
            material: app_res.none_col.clone(),
            ..Default::default()
        })
        .id();
    let homepage_span = commands
        .spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Percent(10.0), Val::Percent(100.0)),
                flex_grow: 100.0,
                ..Default::default()
            },
            material: app_res.none_col.clone(),
            ..Default::default()
        })
        .id();
    let liveroom = commands
        .spawn_bundle(ButtonBundle {
            style: Style {
                size: Size::new(Val::Auto, Val::Percent(100.0)),
                ..Default::default()
            },
            material: app_res.none_col.clone(),
            ..Default::default()
        })
        .insert(ui::following::LiveRoomOpenButton(uid, String::new()))
        .id();
    let liveroom_layout = commands
        .spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Auto, Val::Percent(30.0)),
                flex_direction: FlexDirection::Row,
                ..Default::default()
            },
            material: app_res.none_col.clone(),
            ..Default::default()
        })
        .id();
    let liveroom_span = commands
        .spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Percent(10.0), Val::Percent(100.0)),
                flex_grow: 100.0,
                ..Default::default()
            },
            material: app_res.none_col.clone(),
            ..Default::default()
        })
        .id();

    commands.entity(homepage)
        .insert(ui::following::HoverPressShow(item_layout))
        .push_children(&[nickname]);
    commands.entity(homepage_layout)
        .push_children(&[homepage, homepage_span]);

    commands.entity(liveroom)
        .insert(ui::following::HoverPressShow(item_layout))
        .push_children(&[livetitle]);
    commands.entity(liveroom_layout)
        .push_children(&[liveroom, liveroom_span]);

    commands.entity(description_layout)
        .push_children(&[homepage_layout, videoinfo, liveroom_layout]);
    commands.entity(item_layout)
        .insert(ui::following::HoverPressShower(uid))
        .push_children(&[face, description_layout]);

    commands
        .spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Percent(100.), Val::Percent(20.)),
                flex_direction: FlexDirection::Column,
                ..Default::default()
            },
            material: app_res.none_col.clone(),
            ..Default::default()
        })
        .push_children(&[item_layout, span])
        .id()
}
