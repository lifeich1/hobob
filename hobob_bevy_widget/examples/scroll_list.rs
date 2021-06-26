use bevy::prelude::*;
use hobob_bevy_widget::scroll::{ScrollSimListWidget, ScrollWidgetsPlugin};

fn main() {
    App::build()
        .add_plugins(DefaultPlugins)
        .add_plugin(ScrollWidgetsPlugin())
        //.init_resource::<EnFont>()
        .add_startup_system(setup.system())
        .run();
}

//#[derive(Default)]
//struct EnFont(Handle<Font>);

fn setup(
    mut commands: Commands,
    //mut en_font: ResMut<EnFont>,
    //asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    commands.spawn_bundle(UiCameraBundle::default());

    let bg_col = materials.add(Color::rgb(0.95, 0.15, 0.15).into());
    let list_bg_col = materials.add(Color::rgb(0.16, 0.16, 0.16).into());
    let item_col = materials.add(Color::rgb(0., 0., 0.25).into());

    let root = commands
        .spawn_bundle(NodeBundle {
            style: Style {
                size: Size::new(Val::Percent(100.0), Val::Percent(100.0)),
                justify_content: JustifyContent::Center,
                ..Default::default()
            },
            material: bg_col.clone(),
            ..Default::default()
        })
        .id();

    let mut w = ScrollSimListWidget::with_show_limit(4);

    w.items.push(
        commands
            .spawn_bundle(NodeBundle {
                style: Style {
                    size: Size::new(Val::Px(200.0), Val::Px(20.0)),
                    margin: Rect::all(Val::Px(5.0)),
                    ..Default::default()
                },
                material: item_col.clone(),
                ..Default::default()
            })
            .id(),
    );

    w.items.push(
        commands
            .spawn_bundle(NodeBundle {
                style: Style {
                    size: Size::new(Val::Px(200.0), Val::Px(20.0)),
                    margin: Rect::all(Val::Px(5.0)),
                    ..Default::default()
                },
                material: item_col.clone(),
                ..Default::default()
            })
            .id(),
    );

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
            material: list_bg_col.clone(),
            ..Default::default()
        })
        .insert(w)
        .push_children(&[])
        .id();

    commands.entity(root).push_children(&[list]);
}
