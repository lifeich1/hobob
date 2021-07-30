use super::*;
use hobob_bevy_widget::{button, scroll};

pub fn ui(mut commands: Commands, app_res: Res<AppResource>, cf: Res<AppConfig>) {
    commands.spawn_bundle(UiCameraBundle::default());

    let default_button_bg = button::ButtonBackgroundGroup {
        clicked: app_res.btn_press_col.clone(),
        hovered: app_res.btn_hover_col.clone(),
        none: app_res.btn_none_col.clone(),
    };

    let root = commands
        .spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Percent(100.0), Val::Percent(100.0)),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            material: app_res.bg_col.clone(),
            ..Default::default()
        })
        .id();

    if let Some(e) = cf.startup_error.as_ref() {
        commands.entity(root).with_children(|parent| {
            parent.spawn_bundle(TextBundle {
                text: Text::with_section(
                    format!("STARTUP ERROR: {}", e),
                    TextStyle {
                        font: app_res.font.clone(),
                        font_size: 30.,
                        color: app_res.err_text_col,
                    },
                    TextAlignment {
                        horizontal: HorizontalAlign::Center,
                        ..Default::default()
                    },
                ),
                ..Default::default()
            });
        });
        return;
    }

    let followings: Vec<Entity> = cf
        .followings_uid
        .iter()
        .map(|uid| widget::create_following(&mut commands, &app_res, *uid))
        .collect();

    commands.entity(root).with_children(|parent| {
        parent
            .spawn_bundle(NodeBundle {
                style: Style {
                    size: Size::new(Val::Percent(100.0), Val::Px(35.0)),
                    margin: Rect::all(Val::Px(8.)),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    flex_direction: FlexDirection::Row,
                    ..Default::default()
                },
                material: app_res.none_col.clone(),
                ..Default::default()
            })
            .with_children(|parent| {
                parent
                    .spawn_bundle(ButtonBundle {
                        style: Style {
                            size: Size::new(Val::Px(100.0), Val::Percent(100.0)),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            margin: Rect {
                                right: Val::Px(8.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        material: app_res.btn_none_col.clone(),
                        ..Default::default()
                    })
                    .insert(ui::filter::ReorderButton(ui::filter::Filter::VideoPub))
                    .insert(default_button_bg.clone())
                    .with_children(|parent| {
                        parent.spawn_bundle(TextBundle {
                            text: Text::with_section(
                                "By Video Pubdate",
                                TextStyle {
                                    font: app_res.font.clone(),
                                    font_size: 15.0,
                                    color: app_res.btn_text_col,
                                },
                                Default::default(),
                            ),
                            ..Default::default()
                        });
                    });
                parent
                    .spawn_bundle(ButtonBundle {
                        style: Style {
                            size: Size::new(Val::Px(100.0), Val::Percent(100.0)),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            margin: Rect {
                                right: Val::Px(8.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        material: app_res.btn_none_col.clone(),
                        ..Default::default()
                    })
                    .insert(ui::filter::ReorderButton(ui::filter::Filter::LiveEntropy))
                    .insert(default_button_bg.clone())
                    .with_children(|parent| {
                        parent.spawn_bundle(TextBundle {
                            text: Text::with_section(
                                "By Live Entropy",
                                TextStyle {
                                    font: app_res.font.clone(),
                                    font_size: 15.0,
                                    color: app_res.btn_text_col,
                                },
                                Default::default(),
                            ),
                            ..Default::default()
                        });
                    });
                parent
                    .spawn_bundle(NodeBundle {
                        style: Style {
                            size: Size::new(Val::Px(300.0), Val::Percent(100.0)),
                            align_items: AlignItems::Center,
                            flex_direction: FlexDirection::Row,
                            ..Default::default()
                        },
                        material: app_res.textedit_bg_col.clone(),
                        ..Default::default()
                    })
                    .with_children(|parent| {
                        parent
                            .spawn_bundle(TextBundle {
                                text: Text::with_section(
                                    "",
                                    TextStyle {
                                        font: app_res.font.clone(),
                                        font_size: 25.0,
                                        color: Color::BLACK,
                                    },
                                    Default::default(),
                                ),
                                ..Default::default()
                            })
                            .insert(ui::add::AddFollowing());
                    });
                parent
                    .spawn_bundle(ButtonBundle {
                        style: Style {
                            size: Size::new(Val::Px(50.0), Val::Percent(100.0)),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            margin: Rect {
                                right: Val::Px(4.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        material: app_res.btn_none_col.clone(),
                        ..Default::default()
                    })
                    .insert(ui::add::AddFollowingButton())
                    .insert(default_button_bg.clone())
                    .with_children(|parent| {
                        parent.spawn_bundle(TextBundle {
                            text: Text::with_section(
                                "Add",
                                TextStyle {
                                    font: app_res.font.clone(),
                                    font_size: 15.0,
                                    color: app_res.btn_text_col,
                                },
                                Default::default(),
                            ),
                            ..Default::default()
                        });
                    });
                parent
                    .spawn_bundle(ButtonBundle {
                        style: Style {
                            size: Size::new(Val::Px(100.0), Val::Percent(100.0)),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..Default::default()
                        },
                        material: app_res.btn_none_col.clone(),
                        ..Default::default()
                    })
                    .insert(default_button_bg.clone())
                    .insert(ui::add::RefreshVisible())
                    .with_children(|parent| {
                        parent.spawn_bundle(TextBundle {
                            text: Text::with_section(
                                "Refresh",
                                TextStyle {
                                    font: app_res.font.clone(),
                                    font_size: 15.0,
                                    color: app_res.btn_text_col,
                                },
                                Default::default(),
                            ),
                            ..Default::default()
                        });
                    });
            });

        // span between list & dashboard
        parent.spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Auto, Val::Px(40.)),
                ..Default::default()
            },
            material: app_res.none_col.clone(),
            ..Default::default()
        });

        // followings browser
        parent
            .spawn_bundle(NodeBundle {
                style: Style {
                    size: Size::new(Val::Percent(80.0), Val::Percent(80.0)),
                    flex_direction: FlexDirection::ColumnReverse,
                    flex_grow: 100.0,
                    padding: Rect::all(Val::Px(8.)),
                    ..Default::default()
                },
                material: app_res.none_col.clone(),
                ..Default::default()
            })
            .insert(scroll::ScrollSimListWidget::default())
            .insert(scroll::ScrollProgression::default())
            .push_children(&followings);

        parent
            .spawn_bundle(TextBundle {
                style: Style {
                    size: Size::new(Val::Auto, Val::Auto),
                    position_type: PositionType::Absolute,
                    position: Rect {
                        right: Val::Px(10.0),
                        bottom: Val::Px(10.0),
                        ..Default::default()
                    },
                    border: Rect::all(Val::Px(20.0)),
                    ..Default::default()
                },
                text: Text::with_section(
                    "0%",
                    TextStyle {
                        font: app_res.font.clone(),
                        font_size: app_res.progression_font_size,
                        color: app_res.progression_text_col,
                    },
                    TextAlignment {
                        horizontal: HorizontalAlign::Center,
                        ..Default::default()
                    },
                ),
                ..Default::default()
            })
            .insert(ui::ShowScrollProgression {});
    });
}
