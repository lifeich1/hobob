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
                size: Size::new(Val::Auto, Val::Percent(30.0)),
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
                size: Size::new(Val::Auto, Val::Percent(30.0)),
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

    commands.entity(description_layout)
        .push_children(&[nickname, videoinfo, livetitle]);
    commands.entity(item_layout)
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
