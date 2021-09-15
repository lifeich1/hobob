use error_chain::error_chain;

pub mod db;
pub mod engine;
pub mod www;

error_chain! {
    foreign_links {
        Db(rusqlite::Error);
        BiliApi(bilibili_api_rs::error::ApiError);
    }
}
