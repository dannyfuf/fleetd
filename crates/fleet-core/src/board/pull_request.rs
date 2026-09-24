//! Parsing GitHub pull request references into [`PullRequestRef`].

use super::{model::PullRequestRef, ops::BoardError};
use crate::ids::RepoId;

#[cfg(test)]
mod tests;

/// The one refusal every unparsable reference gets.
const REFUSAL: &str = "expected a GitHub pull request URL or owner/name#number";

impl PullRequestRef {
    /// Parses `https://github.com/<owner>/<name>/pull/<n>` or `<owner>/<name>#<n>` into a
    /// normalised reference.
    ///
    /// The URL form may use `http://` or `www.` and may carry anything after the number
    /// (`/files`, `/commits/…`, `#…`, `?…`); the stored URL is always rebuilt as
    /// `https://github.com/<owner>/<name>/pull/<n>`.
    ///
    /// # Errors
    /// [`BoardError::Invalid`] on `pull_request` when `text` is neither form.
    pub fn parse(text: &str) -> Result<PullRequestRef, BoardError> {
        let text = text.trim();
        let (owner, name, number) = match github_url_path(text) {
            Some(path) => split_url_path(path),
            None => split_short_form(text),
        }
        .ok_or_else(refusal)?;
        let number = Some(number)
            .filter(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|digits| digits.parse::<u64>().ok())
            .filter(|number| *number > 0)
            .ok_or_else(refusal)?;
        let repo = RepoId::try_from(format!("{owner}/{name}")).map_err(|_| refusal())?;
        let pull_request = PullRequestRef {
            url: format!("https://github.com/{repo}/pull/{number}"),
            repo,
            number,
        };
        pull_request.validate()?;
        Ok(pull_request)
    }

    /// Checks the invariants shared by parsed and structured pull request references.
    pub(crate) fn validate(&self) -> Result<(), BoardError> {
        if self.number == 0 {
            return Err(refusal());
        }
        RepoId::try_from(self.repo.as_str()).map_err(|_| refusal())?;
        let expected = format!("https://github.com/{}/pull/{}", self.repo, self.number);
        if self.url != expected {
            return Err(refusal());
        }
        Ok(())
    }
}

/// The path after `github.com/` when `text` is a GitHub URL.
fn github_url_path(text: &str) -> Option<&str> {
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))?;
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    rest.strip_prefix("github.com/")
}

/// `<owner>/<name>/pull/<n>[/…|#…|?…]` into its three parts.
fn split_url_path(path: &str) -> Option<(&str, &str, &str)> {
    let mut segments = path.splitn(4, '/');
    let owner = segments.next()?;
    let name = segments.next()?;
    if segments.next()? != "pull" {
        return None;
    }
    let tail = segments.next()?;
    let number = tail.split(['/', '#', '?']).next()?;
    Some((owner, name, number))
}

/// `<owner>/<name>#<n>` into its three parts.
fn split_short_form(text: &str) -> Option<(&str, &str, &str)> {
    let (repo, number) = text.split_once('#')?;
    let (owner, name) = repo.split_once('/')?;
    Some((owner, name, number))
}

fn refusal() -> BoardError {
    BoardError::Invalid {
        field: "pull_request".into(),
        reason: REFUSAL.into(),
    }
}
