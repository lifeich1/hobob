use bevy::prelude::*;

pub struct AppResource {
    pub err_text_col: Color,
    pub progression_text_col: Color,

    pub none_col: Handle<ColorMaterial>,
    pub bg_col: Handle<ColorMaterial>,
    pub item_bg_col: Handle<ColorMaterial>,
    pub item_to_jump_bg_col: Handle<ColorMaterial>,
    pub textedit_bg_col: Handle<ColorMaterial>,

    pub face_none_img: Handle<ColorMaterial>,

    pub btn_press_col: Handle<ColorMaterial>,
    pub btn_hover_col: Handle<ColorMaterial>,
    pub btn_none_col: Handle<ColorMaterial>,

    pub btn_text_col: Color,

    pub live_on_text: String,
    pub live_off_text: String,
    pub no_video_text: String,

    pub font: Handle<Font>,
    pub progression_font_size: f32,
}

impl FromWorld for AppResource {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.get_resource::<AssetServer>().unwrap();
        let mut font: Handle<Font> = asset_server.load("fonts/FiraMono-Medium.ttf");

        let mut live_on_text = String::from("LIVE ON");
        let mut live_off_text = String::from("LIVE OFF");
        let mut no_video_text = String::from("no videos");

        if std::path::Path::new("assets/fonts/SourceHanSans-Bold.otf").is_file() {
            info!("use SourceHanSans-Bold");
            font = asset_server.load("fonts/SourceHanSans-Bold.otf");

            live_on_text = String::from("直播中");
            live_off_text = String::from("未直播");
            no_video_text = String::from("无上传视频");
        }

        let mut materials = world.get_resource_mut::<Assets<ColorMaterial>>().unwrap();
        Self {
            err_text_col: Color::RED,
            progression_text_col: Color::YELLOW,
            none_col: materials.add(Color::NONE.into()),
            bg_col: materials.add(Color::hex("90d7ec").unwrap().into()),
            item_bg_col: materials.add(Color::hex("7bbfea").unwrap().into()),
            item_to_jump_bg_col: materials.add(Color::WHITE.into()),
            textedit_bg_col: materials.add(Color::WHITE.into()),
            btn_press_col: materials.add(Color::hex("2e3a1f").unwrap().into()),
            btn_hover_col: materials.add(Color::hex("726930").unwrap().into()),
            btn_none_col: materials.add(Color::hex("87843b").unwrap().into()),
            btn_text_col: Color::hex("181d4b").unwrap(),
            face_none_img: materials.add(Color::WHITE.into()),
            progression_font_size: 25.,
            font,
            live_on_text,
            live_off_text,
            no_video_text,
        }
    }
}
