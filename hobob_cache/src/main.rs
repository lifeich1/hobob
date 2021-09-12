use hobob_cache::www;

/*
macro_rules! render {
    ($tera:ident, $name:expr, $ctx:expr) => {
        warp::reply::html(
            $tera.render($name, $ctx)
            .unwrap_or_else(|e| {
                let mut ctx = TeraContext::new();
                ctx.insert("kind", "Tera engine");
                ctx.insert("reason", &format!("Error: tera: {}", e));
                $tera.render("failure.html", &ctx).unwrap()
            })
        )
    }
}
*/

#[tokio::main]
async fn main() {
    env_logger::init();

    www::run().await;
    /*
    let tera0 = match Tera::new("templates/**/
    *.html") {
        Ok(t) => t,
        Err(e) => {
            println!("Parsing error(s): {}", e);
            ::std::process::exit(1);
        }
    };

    let hi = warp::path("hello")
        .and(warp::path::param())
        .map(|param: String| {
            println!("Hello from {}", param);
            format!("Hi {}, you are welcome!", param)
        });

    let tera = tera0.clone();
    let index = warp::path::end()
        .map(move || {
            render!(tera, "index.html", &TeraContext::new())
        });

    let tera = tera0.clone();
    let errtest = warp::path::path("errtest")
        .map(move || {
            render!(tera, "should_not_found.html", &TeraContext::new())
        });

    let app = hi
        .or(index)
        .or(errtest);
    println!("running");
    warp::serve(app).run(([127, 0, 0, 1], 3000)).await;
    */
}
