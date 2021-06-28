use bevy::{app::AppExit, input::mouse, prelude::*};
use hobob_bevy_widget::scroll::{ScrollProgression, ScrollSimListWidget, ScrollWidgetsPlugin};

fn main() {
    App::build()
        .add_plugins(DefaultPlugins)
        .add_plugin(ScrollWidgetsPlugin())
        .init_resource::<AppConfig>()
        .add_startup_system(setup.system())
        .add_system(handle_input.system())
        .add_system(watch_scroll.system())
        .run();
}

struct ShowScrollProgressionNode {}

fn watch_scroll(
    scroll_query: Query<&ScrollProgression, Changed<ScrollProgression>>,
    mut show_nodes: Query<&mut Text, With<ShowScrollProgressionNode>>,
    cf: Res<AppConfig>,
) {
    for progression in scroll_query.iter().take(1) {
        debug!("scroll progression changed: {}", progression.0);
        for mut text in show_nodes.iter_mut() {
            *text = Text::with_section(
                format!("{}%", progression.0),
                TextStyle {
                    font: cf.en_font.clone(),
                    font_size: 25.,
                    color: Color::YELLOW,
                },
                TextAlignment {
                    horizontal: HorizontalAlign::Center,
                    ..Default::default()
                },
            );
        }
    }
}

fn handle_input(
    mut event_exit: EventWriter<AppExit>,
    mut wheel: EventReader<mouse::MouseWheel>,
    keyboard: Res<Input<KeyCode>>,
    mut list_query: Query<&mut ScrollSimListWidget>,
) {
    if keyboard.just_released(KeyCode::Escape) {
        event_exit.send(AppExit {});
    }

    let mut step: i32 = 0;
    if keyboard.just_released(KeyCode::Up) {
        step = -1;
    } else if keyboard.just_released(KeyCode::Down) {
        step = 1;
    } else if keyboard.just_released(KeyCode::Left) {
        step = -3;
    } else if keyboard.just_released(KeyCode::Right) {
        step = 3;
    } else {
        for ev in wheel.iter() {
            if ev.y.abs() > f32::EPSILON {
                debug!("wheel {:?}", *ev);
                step = ev.y.abs().ceil() as i32;
                if ev.y > 0.0 {
                    step = -step;
                }
            }
        }
    }

    if step != 0 {
        for mut list in list_query.iter_mut() {
            list.scroll_to(step);
        }
    }
}

struct AppConfig {
    bg_col: Handle<ColorMaterial>,
    list_bg_col: Handle<ColorMaterial>,
    item_col: Handle<ColorMaterial>,
    en_font: Handle<Font>,
}

impl FromWorld for AppConfig {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.get_resource::<AssetServer>().unwrap();
        let en_font: Handle<Font> = asset_server.load("fonts/FiraMono-Medium.ttf");

        let mut materials = world.get_resource_mut::<Assets<ColorMaterial>>().unwrap();
        Self {
            bg_col: materials.add(Color::rgb(0.15, 0.15, 0.15).into()),
            list_bg_col: materials.add(Color::rgb(0.16, 0.16, 0.16).into()),
            item_col: materials.add(Color::rgb(0.50, 0.50, 0.85).into()),
            en_font,
        }
    }
}

fn setup(mut commands: Commands, cf: Res<AppConfig>) {
    commands.spawn_bundle(UiCameraBundle::default());

    let root = commands
        .spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Percent(100.0), Val::Percent(100.0)),
                justify_content: JustifyContent::Center,
                ..Default::default()
            },
            material: cf.bg_col.clone(),
            ..Default::default()
        })
        .id();

    let mut children = Vec::<Entity>::new();
    for i in 0..10 {
        children.push(
            commands
                .spawn_bundle(NodeBundle {
                    style: Style {
                        size: Size::new(Val::Px(200.0), Val::Px(20.0)),
                        margin: Rect::all(Val::Px(5.0)),
                        ..Default::default()
                    },
                    material: cf.item_col.clone(),
                    ..Default::default()
                })
                .with_children(|parent| {
                    parent.spawn_bundle(TextBundle {
                        text: Text::with_section(
                            format!("item {}", i),
                            TextStyle {
                                font: cf.en_font.clone(),
                                font_size: 15.,
                                color: Color::RED,
                            },
                            TextAlignment {
                                horizontal: HorizontalAlign::Center,
                                ..Default::default()
                            },
                        ),
                        ..Default::default()
                    });
                })
                .id(),
        );
    }

    let list = commands
        .spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Percent(80.0), Val::Percent(100.0)),
                flex_direction: FlexDirection::ColumnReverse,
                align_items: AlignItems::Center,
                padding: Rect {
                    top: Val::Px(40.),
                    ..Default::default()
                },
                ..Default::default()
            },
            material: cf.list_bg_col.clone(),
            ..Default::default()
        })
        .insert(ScrollSimListWidget::with_show_limit(4))
        .insert(ScrollProgression::default())
        .push_children(&children[..])
        .id();

    commands.entity(root).push_children(&[list]);

    let progression = commands
        .spawn_bundle(TextBundle {
            style: Style {
                size: Size::new(Val::Auto, Val::Auto),
                position_type: PositionType::Absolute,
                position: Rect {
                    left: Val::Px(10.0),
                    bottom: Val::Px(10.0),
                    ..Default::default()
                },
                border: Rect::all(Val::Px(20.0)),
                ..Default::default()
            },
            text: Text::with_section(
                "PH",
                TextStyle {
                    font: cf.en_font.clone(),
                    font_size: 25.,
                    color: Color::YELLOW,
                },
                TextAlignment {
                    horizontal: HorizontalAlign::Center,
                    ..Default::default()
                },
            ),
            ..Default::default()
        })
        .insert(ShowScrollProgressionNode {})
        .id();
    commands.entity(root).push_children(&[progression]);
}
