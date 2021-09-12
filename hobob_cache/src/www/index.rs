use warp::{Filter, Rejection, reply::Html};
use crate::www;
use tera::{Context as TeraContext};

pub struct App {}

impl App {
    pub fn build(ctx: www::Context) -> impl www::AppFilter + Clone {
        let tera = ctx.tera.clone();
        warp::path::end()
            .map(move || {
                render!(tera, "index.html", &TeraContext::new())
            })
    }
}
