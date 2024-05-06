//! # Static file server, from inside the binary
//!
//! Acts as a bridge between `include_dir` and `rocket`, enabling you
//! to serve files directly out of the binary executable.
//!
//! See [`StaticFiles`] for more details.

use std::path::PathBuf;

use rocket::{
    fs::Options,
    http::{
        ext::IntoOwned,
        uri::{fmt::Path, Segments},
        ContentType, Method, Status,
    },
    outcome::IntoOutcome,
    response::{self, Redirect, Responder},
    route::{Handler, Outcome},
    Data, Request, Route,
};

pub use include_dir::include_dir;
pub use include_dir::Dir;

/// Implements a simple bridge between `include_dir` and `rocket`. A simple reponder based on
/// [`rocket::FileServer`], which uses a directory included at compile time.
///
/// ```rust
/// use rocket_include_dir::{include_dir, Dir, StaticFiles};
/// #[rocket::launch]
/// fn launch() -> _ {
///     static PROJECT_DIR: Dir = include_dir!("static");
///     build().mount("/", StaticFiles::from(&PROJECT_DIR))
/// }
/// # use rocket::{build, local::blocking::Client, http::Status};
/// # let client = Client::tracked(launch()).expect("valid rocket instance");
/// # let response = client.get("/test-doesnt-exist").dispatch();
/// # assert_eq!(response.status(), Status::NotFound);
/// # let response = client.get("/test.txt").dispatch();
/// # assert_eq!(response.status(), Status::Ok);
/// ```
#[derive(Clone, Copy)]
pub struct StaticFiles {
    dir: &'static Dir<'static>,
    options: Options,
    rank: isize,
}

impl From<&'static Dir<'static>> for StaticFiles {
    fn from(dir: &'static Dir<'static>) -> Self {
        Self {
            dir,
            options: Options::default(),
            rank: Self::DEFAULT_RANK,
        }
    }
}

impl StaticFiles {
    const DEFAULT_RANK: isize = 10;

    /// Construct a new `StaticFiles`, with the provided options.
    ///
    /// The generated route has a default rank of `10`, to match Rocket's
    /// `FileServer`
    pub fn new(dir: &'static Dir<'static>, options: Options) -> Self {
        Self {
            dir,
            options,
            rank: Self::DEFAULT_RANK,
        }
    }

    /// Replace the options for this `StaticFiles`
    pub fn options(mut self, options: Options) -> Self {
        self.options = options;
        self
    }

    /// Set a non-default rank for this `StaticFiles`
    pub fn rank(mut self, rank: isize) -> Self {
        self.rank = rank;
        self
    }
}

fn respond_with<'r>(
    req: &'r Request<'_>,
    path: PathBuf,
    contents: &Dir<'r>,
) -> response::Result<'r> {
    let response = contents.get_file(&path).ok_or(Status::NotFound)?;
    let mut response = response.contents().respond_to(req)?;
    if let Some(ext) = path.extension() {
        if let Some(ct) = ContentType::from_extension(&ext.to_string_lossy()) {
            response.set_header(ct);
        }
    }

    Ok(response)
}

#[rocket::async_trait]
impl Handler for StaticFiles {
    async fn handle<'r>(&self, req: &'r Request<'_>, data: Data<'r>) -> Outcome<'r> {
        // TODO: Should we reject dotfiles for `self.root` if !DotFiles?
        let options = self.options;
        // Get the segments as a `PathBuf`, allowing dotfiles requested.
        let allow_dotfiles = options.contains(Options::DotFiles);
        let path = req
            .segments::<Segments<'_, Path>>(0..)
            .ok()
            .and_then(|segments| segments.to_path_buf(allow_dotfiles).ok());

        match path {
            Some(p) if p.is_dir() => {
                // Normalize '/a/b/foo' to '/a/b/foo/'.
                if options.contains(Options::NormalizeDirs) && !req.uri().path().ends_with('/') {
                    let normal = req
                        .uri()
                        .map_path(|p| format!("{}/", p))
                        .expect("adding a trailing slash to a known good path => valid path")
                        .into_owned();

                    return Redirect::permanent(normal)
                        .respond_to(req)
                        .or_forward((data, Status::InternalServerError));
                }

                if !options.contains(Options::Index) {
                    return Outcome::forward(data, Status::NotFound);
                }

                respond_with(req, p.join("index.html"), &self.dir)
                    .or_forward((data, Status::NotFound))
            }
            Some(p) => respond_with(req, p, &self.dir).or_forward((data, Status::NotFound)),
            None => Outcome::forward(data, Status::NotFound),
        }
    }
}

impl Into<Route> for StaticFiles {
    fn into(self) -> Route {
        Route::ranked(self.rank, Method::Get, "/<path..>", self)
    }
}

impl Into<Vec<Route>> for StaticFiles {
    fn into(self) -> Vec<Route> {
        vec![self.into()]
    }
}

#[cfg(test)]
mod tests {
    use include_dir::include_dir;
    use rocket::{build, local::blocking::Client, Build, Rocket};

    use super::*;

    fn launch() -> Rocket<Build> {
        static PROJECT_DIR: Dir = include_dir!("static");
        build().mount("/", StaticFiles::new(&PROJECT_DIR, Options::default()))
    }

    #[test]
    fn it_works() {
        let client = Client::tracked(launch()).expect("valid rocket instance");
        let response = client.get("/test-doesnt-exist").dispatch();
        assert_eq!(response.status(), Status::NotFound);
        let response = client.get("/test.txt").dispatch();
        assert_eq!(response.status(), Status::Ok);
    }
}
