use bevy::prelude::*;

pub struct ShowScrollProgression {}

pub mod add {
    pub struct RefreshVisible();
    pub struct AddFollowing();
    pub struct AddFollowingButton();
}

pub mod filter {
    pub enum Filter {
        LiveEntropy,
        VideoPub,
    }

    pub struct ReorderButton(pub Filter);
}

pub mod following {
    use bevy::prelude::*;

    pub struct Nickname(pub u64);
    #[derive(Debug)]
    pub struct HomepageOpenButton(pub u64);
    pub struct Face(pub u64);
    #[derive(Debug)]
    pub struct LiveRoomOpenButton(pub u64, pub String);
    pub struct LiveRoomTitle(pub u64);
    pub struct VideoInfo(pub u64);

    pub struct HoverPressShow(pub Entity);
    pub struct HoverPressShower(pub u64);

    pub mod data {
        use bevy::prelude::*;

        #[derive(Debug)]
        pub struct Uid(pub u64);

        #[derive(Debug)]
        pub struct Info {
            pub nickname: String,        // Nickname
            pub live_room_url: String,   // LiveRoomOpenButton
            pub live_room_title: String, // LiveRoomTitle
            pub live_open: Option<bool>,
            pub live_entropy: u64,
            pub face_url: String, // request Face
        }

        #[derive(Debug, Default)]
        pub struct SortKey {
            pub live_entropy: u64,
            pub video_pub_ts: u64,
        }

        #[derive(Debug)]
        pub struct Face {
            pub face: Handle<ColorMaterial>, // Face
        }

        #[derive(Debug, Default)]
        pub struct NewVideo {
            pub date_time: String, // VideoInfo
            pub title: String,
            pub timestamp_sec: u64,
        }

        #[derive(Debug)]
        pub enum Data {
            Info(Info),
            Face(Face),
            NewVideo(NewVideo),
        }

        impl Data {
            pub fn as_info(&self) -> Option<&Info> {
                match self {
                    Self::Info(v) => Some(v),
                    _ => None,
                }
            }

            pub fn as_face(&self) -> Option<&Face> {
                match self {
                    Self::Face(v) => Some(v),
                    _ => None,
                }
            }

            pub fn as_new_video(&self) -> Option<&NewVideo> {
                match self {
                    Self::NewVideo(v) => Some(v),
                    _ => None,
                }
            }
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
        app.add_event::<following::event::Action>()
            .add_event::<following::event::ParsedApiResult>();
    }
}
