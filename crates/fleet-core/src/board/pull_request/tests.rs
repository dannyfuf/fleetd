//! Pull request reference parsing tests.

use super::*;

#[test]
fn pull_request_references_parse_and_normalise() {
    let cases = [
        ("https://github.com/acme/api/pull/12", "acme/api", 12),
        ("https://github.com/acme/api/pull/12/files", "acme/api", 12),
        (
            "https://github.com/acme/api/pull/7/commits/abc123",
            "acme/api",
            7,
        ),
        (
            "https://github.com/acme/api/pull/12#issuecomment-1",
            "acme/api",
            12,
        ),
        (
            "https://github.com/acme/api/pull/12?tab=checks",
            "acme/api",
            12,
        ),
        ("http://www.github.com/acme/api/pull/3", "acme/api", 3),
        ("acme/api#12", "acme/api", 12),
        ("  acme/web-app#420 \n", "acme/web-app", 420),
        (
            "\thttps://github.com/acme/api/pull/12/files  ",
            "acme/api",
            12,
        ),
    ];
    for (text, repo, number) in cases {
        let parsed =
            PullRequestRef::parse(text).unwrap_or_else(|error| panic!("{text:?}: {error}"));
        assert_eq!(parsed.repo.as_str(), repo, "{text:?}");
        assert_eq!(parsed.number, number, "{text:?}");
        assert_eq!(
            parsed.url,
            format!("https://github.com/{repo}/pull/{number}"),
            "{text:?}"
        );
        assert_eq!(parsed.key(), format!("{repo}#{number}"), "{text:?}");
    }
}

#[test]
fn pull_request_references_refuse_what_is_not_one() {
    let cases = [
        "",
        "   ",
        "acme/api",
        "#12",
        "https://github.com/acme/api/issues/12",
        "https://gitlab.com/acme/api/pull/1",
        "https://github.com/acme/api/pull/0",
        "acme/api#0",
        "https://github.com/acme/api/pull/twelve",
        "acme/api#12a",
        "acme/api#+12",
        "https://github.com/acme/api/pull/",
        "https://github.com/acme/pull/12",
        "acme/api/more#12",
        "ac me/api#12",
    ];
    for text in cases {
        let error = PullRequestRef::parse(text).expect_err(text);
        assert_eq!(
            error,
            BoardError::Invalid {
                field: "pull_request".into(),
                reason: "expected a GitHub pull request URL or owner/name#number".into(),
            },
            "{text:?}"
        );
    }
}
