//! Group/Square Note (Group Home) commands missing from [`crate::timeline`]:
//! announcements, comment list/delete/report, like cancel, post report and
//! search.
//!
//! Derived from static RE of LINE macOS 26.5.0; unverified against live
//! server. Evidence: `C*` command vtables at `0x105e3a900..0x105e3c668`
//! (method, path, query/body keys) and the server-type selector at
//! `0x1026b9d70` (group homes → `/ext/note/nt`, squares → `/sn`).
//!
//! **Base path.** The desktop client never uses `/mh` for Note: post,
//! comment and like commands go to `/ext/note/nt` (or `/sn` for squares),
//! and announcement commands are hard-wired to `/sn` (server type 25).
//! [`crate::timeline`] keeps linejs' `/mh` routes for posts.
//!
//! **Auth.** The desktop maps commands to channels through a switch
//! (`0x1026ba28c`) whose per-command input was not recovered; these
//! wrappers use the MyHome channel, the one [`crate::timeline`] already
//! uses successfully for `/ext/note/nt` likes and comments.

use serde_json::{Value as Json, json};

use super::{RestAuth, RestClient, RestPrefix, RestRequest, channels};
use crate::error::Result;

const AUTH: RestAuth = RestAuth::Channel(channels::MYHOME);

pub const REPORT_POST: &str = "/api/v57/post/report.json";
pub const SEARCH_NOTE: &str = "/api/v57/search/note.json";
pub const SEARCH_SQUARE_NOTE: &str = "/api/v57/search/note";
pub const CANCEL_LIKE: &str = "/api/v57/like/cancel.json";
pub const DELETE_COMMENT: &str = "/api/v57/comment/delete.json";
pub const LIST_COMMENTS: &str = "/api/v57/comment/getList.json";
pub const REPORT_COMMENT: &str = "/api/v57/comment/report.json";
pub const ANNOUNCED_POSTS: &str = "/api/v57/announce/postlist.json";
pub const CREATE_ANNOUNCEMENT: &str = "/api/v57/announce/create.json";
pub const DELETE_ANNOUNCEMENT: &str = "/api/v57/announce/delete.json";
pub const LIST_ANNOUNCEMENTS: &str = "/api/v57/announce/list.json";

/// Options of [`Note::search`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchNote {
    pub home_id: String,
    pub query: String,
    pub query_type: Option<String>,
    pub scroll_id: Option<String>,
    pub post_limit: Option<i32>,
    pub like_limit: Option<i32>,
    pub comment_limit: Option<i32>,
}

impl SearchNote {
    fn pairs(&self) -> Vec<(&'static str, Json)> {
        let mut v = vec![("query", Json::from(self.query.as_str()))];
        let opt = [
            ("queryType", self.query_type.clone().map(Json::from)),
            ("homeId", Some(Json::from(self.home_id.as_str()))),
            ("scrollId", self.scroll_id.clone().map(Json::from)),
            ("postLimit", self.post_limit.map(Json::from)),
            ("likeLimit", self.like_limit.map(Json::from)),
            ("commentLimit", self.comment_limit.map(Json::from)),
        ];
        v.extend(opt.into_iter().filter_map(|(k, x)| x.map(|x| (k, x))));
        v
    }

    /// Body of the group search (`POST search/note.json`).
    pub fn body(&self) -> Json {
        Json::Object(self.pairs().into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }
}

/// Note operations (see [`RestClient::note`]).
#[derive(Clone, Copy)]
pub struct Note<'a> {
    rest: &'a RestClient,
}

impl RestClient {
    pub fn note(&self) -> Note<'_> {
        Note { rest: self }
    }
}

fn get(home_id: &str, path: &str) -> RestRequest {
    RestRequest::get(RestPrefix::note_for(home_id), path, AUTH).query("homeId", home_id)
}

impl Note<'_> {
    /// `GET post/report.json?homeId&postId&reason`.
    pub async fn report_post(&self, home_id: &str, post_id: &str, reason: &str) -> Result<Json> {
        self.rest.send(get(home_id, REPORT_POST).query("postId", post_id).query("reason", reason)).await
    }

    /// `GET like/cancel.json?homeId&contentId` (content id = post id).
    pub async fn cancel_like(&self, home_id: &str, content_id: &str) -> Result<Json> {
        self.rest.send(get(home_id, CANCEL_LIKE).query("contentId", content_id)).await
    }

    /// `GET comment/getList.json?homeId&actorId&contentId&limit&scrollId`.
    pub async fn list_comments(
        &self,
        home_id: &str,
        content_id: &str,
        actor_id: Option<&str>,
        limit: Option<i32>,
        scroll_id: Option<&str>,
    ) -> Result<Json> {
        let req = get(home_id, LIST_COMMENTS)
            .query_opt("actorId", actor_id)
            .query("contentId", content_id)
            .query_opt("limit", limit)
            .query_opt("scrollId", scroll_id);
        self.rest.send(req).await
    }

    /// `GET comment/delete.json?homeId&commentId&actorId`.
    pub async fn delete_comment(&self, home_id: &str, comment_id: &str, actor_id: Option<&str>) -> Result<Json> {
        self.rest.send(get(home_id, DELETE_COMMENT).query("commentId", comment_id).query_opt("actorId", actor_id)).await
    }

    /// `GET comment/report.json?homeId&commentId&reason`.
    pub async fn report_comment(&self, home_id: &str, comment_id: &str, reason: &str) -> Result<Json> {
        self.rest.send(get(home_id, REPORT_COMMENT).query("commentId", comment_id).query("reason", reason)).await
    }

    /// Search posts. Groups: `POST /ext/note/nt/.../search/note.json?homeId`
    /// with a JSON body; squares: `GET /sn/.../search/note` with the same
    /// keys in the query (`CSearchSquareNote`).
    pub async fn search(&self, opts: SearchNote) -> Result<Json> {
        let req = if opts.home_id.starts_with('s') {
            let mut req = RestRequest::get(RestPrefix::SquareNote, SEARCH_SQUARE_NOTE, AUTH);
            for (k, v) in opts.pairs() {
                req = req.query(k, v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()));
            }
            req
        } else {
            RestRequest::post(RestPrefix::Note, SEARCH_NOTE, AUTH).query("homeId", &opts.home_id).body(opts.body())
        };
        self.rest.send(req).await
    }

    fn sn(home_id: &str, path: &str) -> RestRequest {
        RestRequest::get(RestPrefix::SquareNote, path, AUTH).query("homeId", home_id)
    }

    /// `GET /sn/.../announce/list.json?homeId&limit`.
    pub async fn list_announcements(&self, home_id: &str, limit: Option<i32>) -> Result<Json> {
        self.rest.send(Self::sn(home_id, LIST_ANNOUNCEMENTS).query_opt("limit", limit)).await
    }

    /// `GET /sn/.../announce/postlist.json?homeId&scrollId`.
    pub async fn list_announced_posts(&self, home_id: &str, scroll_id: Option<&str>) -> Result<Json> {
        self.rest.send(Self::sn(home_id, ANNOUNCED_POSTS).query_opt("scrollId", scroll_id)).await
    }

    /// `POST /sn/.../announce/create.json?homeId` `{postId}`.
    pub async fn create_announcement(&self, home_id: &str, post_id: &str) -> Result<Json> {
        let req = RestRequest::post(RestPrefix::SquareNote, CREATE_ANNOUNCEMENT, AUTH)
            .query("homeId", home_id)
            .body(json!({"postId": post_id}));
        self.rest.send(req).await
    }

    /// `GET /sn/.../announce/delete.json?homeId&postId`.
    pub async fn delete_announcement(&self, home_id: &str, post_id: &str) -> Result<Json> {
        self.rest.send(Self::sn(home_id, DELETE_ANNOUNCEMENT).query("postId", post_id)).await
    }
}
