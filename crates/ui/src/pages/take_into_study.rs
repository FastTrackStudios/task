//! "Take into my study" — a page crosses from a working wiki into a
//! curated one, from the page itself.
//!
//! The Bible Study Library is written by an agent; a person's own Bible
//! Study holds only what they chose to bring in once they had studied it
//! enough. Bringing a page in is a *promotion* (`wiki.promote.*`): a
//! copy, recorded on both ends, refused rather than guessed when the
//! target does not hold the page's type, never overwriting a page that
//! is already there. The reasoning lives with the planner
//! ([`wiki_proto::promote`]); this is the same four calls `task wiki
//! promote` makes, in the same order, from the app.

use std::collections::BTreeSet;

use wiki_proto::promote::{self, PromoteRequest};
use wiki_proto::service::pages::PagesClient;
use wiki_proto::service::schema::SchemaClient;

/// Copy `from_wiki::path` into `to_wiki`. Returns where it landed.
///
/// # Errors
///
/// A plain sentence for the page: the target already holds the page, the
/// target's schema does not declare the page's type (with the types it
/// does declare), or a read or write failed.
pub(crate) async fn take_into(
    slug: &str,
    from_wiki: &str,
    path: &str,
    to_wiki: &str,
) -> Result<String, String> {
    let pages = crate::vox_clients::establish_for::<PagesClient>(slug).await?;
    let schemas = crate::vox_clients::establish_for::<SchemaClient>(slug).await?;
    let source = pages
        .read_page(from_wiki.to_owned(), path.to_owned())
        .await
        .map_err(|e| format!("Couldn't read the page: {e:?}"))?;
    let schema = schemas.read_schema(to_wiki.to_owned()).await.map_err(|_| {
        "That wiki has no schema, so it can't say which kinds of page it holds.".to_owned()
    })?;
    let target_names: BTreeSet<String> = pages
        .list_pages(to_wiki.to_owned())
        .await
        .map_err(|e| format!("Couldn't list that wiki's pages: {e:?}"))?
        .into_iter()
        .flat_map(|p| {
            let stem = p
                .path
                .rsplit('/')
                .next()
                .and_then(|f| f.strip_suffix(".md"))
                .unwrap_or_default()
                .to_owned();
            [p.title, stem]
        })
        .filter(|n| !n.is_empty())
        .collect();
    let plan = promote::plan(
        &source.markdown,
        &PromoteRequest {
            from_wiki,
            from_path: path,
            to_wiki,
            to_path: None,
            as_type: None,
            target_schema: &schema.markdown,
            target_names: &target_names,
            at: chrono::Utc::now(),
        },
    )
    .map_err(|e| e.to_string())?;
    // Refuse before writing anything when the target already holds it:
    // a study page is never overwritten by the page it came from.
    match pages
        .read_page(to_wiki.to_owned(), plan.to_path.clone())
        .await
    {
        Ok(_) => {
            return Err(format!(
                "Your study already has {} — open it there, or move it aside first.",
                plan.to_path
            ));
        }
        Err(vox::VoxError::User(e)) if matches!(*e, wiki_proto::WikiError::NotFound(_)) => {}
        Err(e) => return Err(format!("Couldn't check whether it's already there: {e:?}")),
    }
    // The copy first, then the back-reference: a failure between them
    // leaves a study page that knows where it came from, never a library
    // page claiming a copy that does not exist.
    pages
        .write_page(
            to_wiki.to_owned(),
            plan.to_path.clone(),
            plan.promoted_markdown.clone(),
            String::new(),
        )
        .await
        .map_err(|e| format!("Couldn't write it into your study: {e:?}"))?;
    pages
        .write_page(
            from_wiki.to_owned(),
            path.to_owned(),
            plan.annotated_source.clone(),
            source.sha256.clone(),
        )
        .await
        .map_err(|e| {
            format!(
                "It's in your study, but the library page couldn't record that ({e:?}). \
                 Nothing is lost."
            )
        })?;
    Ok(plan.to_path)
}

/// `wiki::Path.md` from a `promoted_from:` / `promoted_to:` value.
#[must_use]
pub(crate) fn split_ref(value: &str) -> Option<(String, String)> {
    let v = value.trim().trim_matches(['"', '\'']);
    let (wiki, path) = v.split_once("::")?;
    (!wiki.is_empty() && !path.is_empty()).then(|| (wiki.to_owned(), path.to_owned()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_promotion_reference_splits() {
        assert_eq!(
            super::split_ref("\"bible-study-library::Topics/Elohim.md\""),
            Some(("bible-study-library".into(), "Topics/Elohim.md".into()))
        );
        assert_eq!(super::split_ref("nothing"), None);
    }
}
