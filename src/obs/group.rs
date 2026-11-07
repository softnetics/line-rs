//! Several images sent at once as one photo group, the way the Android
//! app does (`yq8/n` `MultipleImageContentUploadTask`).
//!
//! Every image goes plain (V1) with `GID`/`GTOTAL`/`GSEQ` in
//! `X-Talk-Meta`. The first one sends `GID` `"0"`; LINE answers with the
//! group's id in `x-line-message-gid` and the others carry that id.

use super::e2ee::{MediaBackend, MediaSend, send_media};
use super::meta::MediaGroup;
use super::{MediaOptions, ObjType, Obs, ObsFile, SentMedia};
use crate::error::{Error, Result};

fn skipped() -> Error {
    Error::internal("ObsError", "Not sent: an earlier image of the group failed")
}

/// Send `images` to `to` as one group; one result per image, in order.
///
/// Like the app, the group stops at the first image that fails and the
/// ones after it are not sent. When LINE names no group after the first
/// image, the rest are sent one by one (the app drops them; unverified
/// why).
pub(crate) async fn send_image_group(
    backend: &dyn MediaBackend,
    to: &str,
    images: Vec<(ObsFile, MediaOptions)>,
) -> Vec<Result<SentMedia>> {
    let total = images.len();
    let mut results = Vec::with_capacity(total);
    let mut images = images.into_iter();
    let single = |file, opts| MediaSend {
        to: to.to_string(),
        o_type: ObjType::Image,
        file,
        opts,
    };
    if total < 2 {
        for (file, opts) in images {
            results.push(send_media(backend, single(file, opts)).await);
        }
        return results;
    }
    let Some((file, opts)) = images.next() else {
        return results;
    };
    let first = send_media(
        backend,
        single(file, opts.group(MediaGroup::first(total as u32))),
    )
    .await;
    let group_id = match &first {
        Ok(SentMedia::Plain {
            group_id: Some(id), ..
        }) => Some(id.clone()),
        Ok(_) => None,
        Err(_) => {
            results.push(first);
            results.extend(images.map(|_| Err(skipped())));
            return results;
        }
    };
    results.push(first);
    let mut failed = false;
    for (i, (file, opts)) in images.enumerate() {
        if failed {
            results.push(Err(skipped()));
            continue;
        }
        let opts = match &group_id {
            Some(id) => opts.group(MediaGroup {
                id: id.clone(),
                total: total as u32,
                seq: i as u32 + 2,
            }),
            None => opts,
        };
        let res = send_media(backend, single(file, opts)).await;
        failed = res.is_err() && group_id.is_some();
        results.push(res);
    }
    results
}

impl Obs<'_> {
    /// Send several images to `to` as one photo group, as the app does
    /// when you pick more than one: always plain (V1) uploads, whatever
    /// the chat's flow. Returns one result per image, in order; after a
    /// failure the rest of the group is not sent.
    pub async fn send_image_group(
        &self,
        to: &str,
        images: Vec<(ObsFile, MediaOptions)>,
    ) -> Vec<Result<SentMedia>> {
        send_image_group(self, to, images).await
    }
}
