use bevy::prelude::*;

pub mod add {
    pub struct RefreshVisible();
    pub struct AddFollowing();
}

pub mod following {
    pub struct Nickname(pub u64);
    pub struct HomepageOpenButton(pub u64);
    pub struct Face(pub u64);
    pub struct LiveRoomOpenButton(pub u64);
    pub struct LiveRoomTitle(pub u64);
    pub struct VideoInfo(pub u64);

    pub mod data {
        use bevy::prelude::*;

        #[derive(Debug)]
        pub struct Info {
            pub nickname: String,        // Nickname
            pub live_room_url: String,   // LiveRoomOpenButton
            pub live_room_title: String, // LiveRoomTitle
            pub live_open: Option<bool>,
            pub live_entropy: u64,
            pub face_url: String, // request Face
        }

        #[derive(Debug)]
        pub struct Face {
            pub face: Handle<ColorMaterial>, // Face
        }

        #[derive(Debug)]
        pub struct NewVideo {
            pub date_time: String, // VideoInfo
            pub title: String,
        }

        #[derive(Debug)]
        pub enum Data {
            Info(Info),
            Face(Face),
            NewVideo(NewVideo),
        }
    }

    pub mod event {
        use super::data;

        #[derive(Debug)]
        pub enum Action {
            RefreshVisible,
            AddFollowingUid(u64),
        }

        #[derive(Debug)]
        pub struct ParsedApiResult {
            pub uid: u64,
            pub data: data::Data,
        }
    }
}

pub struct ResourcePlugin();

impl Plugin for ResourcePlugin {
    fn build(&self, app: &mut AppBuilder) {
        app
            .add_event::<following::event::Action>()
            .add_event::<following::event::ParsedApiResult>();
    }
}
