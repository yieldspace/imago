use crate::planner::ReleasePlan;

pub fn render_explanation(plan: &ReleasePlan, target: &str) -> String {
    if let Some(line_bump) = plan.line_bumps.iter().find(|line| line.line_id == target) {
        let mut lines = vec![format!(
            "line `{}` は {:?} bump です。",
            line_bump.line_id, line_bump.bump
        )];

        if !line_bump.triggered_by.is_empty() {
            lines.push(format!(
                "直接影響 crate: {}",
                line_bump.triggered_by.join(", ")
            ));
        }
        if !line_bump.propagated_from.is_empty() {
            lines.push(format!(
                "伝播元 line: {}",
                line_bump.propagated_from.join(", ")
            ));
        }
        return lines.join("\n");
    }

    if let Some(crate_update) = plan
        .crate_updates
        .iter()
        .find(|crate_update| crate_update.crate_name == target)
    {
        return format!(
            "crate `{}` は line `{}` に属し、{} -> {} へ更新予定です。",
            crate_update.crate_name, crate_update.line_id, crate_update.before, crate_update.after,
        );
    }

    format!("`{}` は今回の release plan に含まれていません。", target)
}
