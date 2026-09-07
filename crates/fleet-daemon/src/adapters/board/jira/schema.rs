use super::*;

impl JiraBackend {
    /// The issue types a project accepts, for the `jira.issue_type` property's options.
    ///
    /// A site that refuses the project read still has a syncable board, so this is the one
    /// call whose failure is a warning: it costs option labels, not cards.
    pub(super) async fn issue_types(
        &self,
        acli: &Acli,
        settings: &JiraSettings,
    ) -> Vec<PropertyOption> {
        let value = match acli
            .json(&["project", "view", "--key", &settings.project, "--json"])
            .await
        {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(project = %settings.project, %error, "could not read the project's issue types");
                return Vec::new();
            }
        };
        value
            .get("issueTypes")
            .and_then(serde_json::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|issue_type| {
                let name = issue_type.get("name")?.as_str()?.trim();
                (!name.is_empty()).then(|| PropertyOption {
                    value: name.to_owned(),
                    label: name.to_owned(),
                    color: None,
                })
            })
            .collect()
    }

    /// Fills the account cache and the label list from the board's most recently touched issues.
    ///
    /// A board whose columns are configured by hand never samples for statuses, so this is the
    /// only thing that teaches a cold daemon the accounts its next push has to address. It costs
    /// one `search`, and a site that refuses it still has a syncable board.
    pub(super) async fn sample_people(
        &self,
        acli: &Acli,
        settings: &JiraSettings,
        scope: &str,
        labels: &mut Vec<String>,
        assignees: &mut Vec<String>,
    ) {
        let jql = format!("{} ORDER BY updated DESC", settings.base_jql());
        let limit = settings.sample_limit.to_string();
        let sample = match acli
            .json(&[
                "workitem",
                "search",
                "--jql",
                &jql,
                "--fields",
                "labels,assignee",
                "--limit",
                &limit,
                "--json",
            ])
            .await
        {
            Ok(sample) => sample,
            Err(error) => {
                tracing::warn!(%error, "could not sample this board's people and labels");
                return;
            }
        };
        for issue in issues(&sample) {
            self.remember_users(scope, issue);
            let fields = issue.get("fields").unwrap_or(issue);
            for label in fields
                .get("labels")
                .and_then(serde_json::Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter_map(|label| label.as_str())
            {
                remember(labels, label);
            }
            if let Some(assignee) = fields
                .get("assignee")
                .and_then(|assignee| assignee.get("displayName"))
                .and_then(serde_json::Value::as_str)
            {
                remember(assignees, assignee);
            }
        }
    }
}
