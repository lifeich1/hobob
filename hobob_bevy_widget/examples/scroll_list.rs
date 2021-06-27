use bevy::{
    prelude::*,
    app::AppExit,
};
use hobob_bevy_widget::scroll::{ScrollSimListWidget, ScrollWidgetsPlugin};

fn main() {
    App::build()
        .add_plugins(DefaultPlugins)
        .add_plugin(ScrollWidgetsPlugin())
        .init_resource::<AppConfig>()
        .add_startup_system(setup.system())
        .add_system(handle_input.system())
        .run();
}

fn handle_input(
    mut event_exit: EventWriter<AppExit>,
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

    let mut w = ScrollSimListWidget::with_show_limit(4);

    for i in 0..10 {
        w.items.push(
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
                                color: Color::BLACK,
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
        .insert(w)
        .push_children(&[])
        .id();

    commands.entity(root).push_children(&[list]);
}
