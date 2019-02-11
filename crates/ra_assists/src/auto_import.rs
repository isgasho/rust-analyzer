use ra_syntax::{
    ast, AstNode, SyntaxNode, Direction, TextRange,
    SyntaxKind::{ PATH, PATH_SEGMENT, COLONCOLON, COMMA }
};
use crate::assist_ctx::{AssistCtx, Assist, AssistBuilder};

fn collect_path_segments(path: &ast::Path) -> Option<Vec<&ast::PathSegment>> {
    let mut v = Vec::new();
    collect_path_segments_raw(&mut v, path)?;
    return Some(v);
}

fn collect_path_segments_raw<'a>(
    segments: &mut Vec<&'a ast::PathSegment>,
    mut path: &'a ast::Path,
) -> Option<usize> {
    let oldlen = segments.len();
    loop {
        let mut children = path.syntax().children();
        let (first, second, third) = (
            children.next().map(|n| (n, n.kind())),
            children.next().map(|n| (n, n.kind())),
            children.next().map(|n| (n, n.kind())),
        );
        match (first, second, third) {
            (Some((subpath, PATH)), Some((_, COLONCOLON)), Some((segment, PATH_SEGMENT))) => {
                path = ast::Path::cast(subpath)?;
                segments.push(ast::PathSegment::cast(segment)?);
            }
            (Some((segment, PATH_SEGMENT)), _, _) => {
                segments.push(ast::PathSegment::cast(segment)?);
                break;
            }
            (_, _, _) => return None,
        }
    }
    // We need to reverse only the new added segments
    let only_new_segments = segments.split_at_mut(oldlen).1;
    only_new_segments.reverse();
    return Some(segments.len() - oldlen);
}

fn fmt_segments(segments: &[&ast::PathSegment]) -> String {
    let mut buf = String::new();
    fmt_segments_raw(segments, &mut buf);
    return buf;
}

fn fmt_segments_raw(segments: &[&ast::PathSegment], buf: &mut String) {
    let mut first = true;
    for s in segments {
        if !first {
            buf.push_str("::");
        }
        match s.kind() {
            Some(ast::PathSegmentKind::Name(nameref)) => buf.push_str(nameref.text()),
            Some(ast::PathSegmentKind::SelfKw) => buf.push_str("self"),
            Some(ast::PathSegmentKind::SuperKw) => buf.push_str("super"),
            Some(ast::PathSegmentKind::CrateKw) => buf.push_str("crate"),
            None => {}
        }
        first = false;
    }
}

// Returns the numeber of common segments.
fn compare_path_segments(left: &[&ast::PathSegment], right: &[&ast::PathSegment]) -> usize {
    return left.iter().zip(right).filter(|(l, r)| compare_path_segment(l, r)).count();
}

fn compare_path_segment(a: &ast::PathSegment, b: &ast::PathSegment) -> bool {
    if let (Some(ka), Some(kb)) = (a.kind(), b.kind()) {
        match (ka, kb) {
            (ast::PathSegmentKind::Name(nameref_a), ast::PathSegmentKind::Name(nameref_b)) => {
                nameref_a.text() == nameref_b.text()
            }
            (ast::PathSegmentKind::SelfKw, ast::PathSegmentKind::SelfKw) => true,
            (ast::PathSegmentKind::SuperKw, ast::PathSegmentKind::SuperKw) => true,
            (ast::PathSegmentKind::CrateKw, ast::PathSegmentKind::CrateKw) => true,
            (_, _) => false,
        }
    } else {
        false
    }
}

fn compare_path_segment_with_name(a: &ast::PathSegment, b: &ast::Name) -> bool {
    if let Some(ka) = a.kind() {
        return match (ka, b) {
            (ast::PathSegmentKind::Name(nameref_a), _) => nameref_a.text() == b.text(),
            (_, _) => false,
        };
    } else {
        false
    }
}

#[derive(Copy, Clone)]
enum ImportAction<'a> {
    Nothing,
    // Add a brand new use statement.
    AddNewUse {
        anchor: Option<&'a SyntaxNode>, // anchor node
        add_after_anchor: bool,
    },

    // To split an existing use statement creating a nested import.
    AddNestedImport {
        // how may segments matched with the target path
        common_segments: usize,
        path_to_split: &'a ast::Path,
        // the first segment of path_to_split we want to add into the new nested list
        first_segment_to_split: Option<&'a ast::PathSegment>,
        // Wether to add 'self' in addition to the target path
        add_self: bool,
    },
    // To add the target path to an existing nested import tree list.
    AddInTreeList {
        common_segments: usize,
        // The UseTreeList where to add the target path
        tree_list: &'a ast::UseTreeList,
        add_self: bool,
    },
}

impl<'a> ImportAction<'a> {
    fn add_new_use(anchor: Option<&'a SyntaxNode>, add_after_anchor: bool) -> Self {
        ImportAction::AddNewUse { anchor, add_after_anchor }
    }

    fn add_nested_import(
        common_segments: usize,
        path_to_split: &'a ast::Path,
        first_segment_to_split: Option<&'a ast::PathSegment>,
        add_self: bool,
    ) -> Self {
        ImportAction::AddNestedImport {
            common_segments,
            path_to_split,
            first_segment_to_split,
            add_self,
        }
    }

    fn add_in_tree_list(
        common_segments: usize,
        tree_list: &'a ast::UseTreeList,
        add_self: bool,
    ) -> Self {
        ImportAction::AddInTreeList { common_segments, tree_list, add_self }
    }

    fn better<'b>(left: &'b ImportAction<'a>, right: &'b ImportAction<'a>) -> &'b ImportAction<'a> {
        if left.is_better(right) {
            left
        } else {
            right
        }
    }

    fn is_better(&self, other: &ImportAction) -> bool {
        match (self, other) {
            (ImportAction::Nothing, _) => true,
            (ImportAction::AddInTreeList { .. }, ImportAction::Nothing) => false,
            (
                ImportAction::AddNestedImport { common_segments: n, .. },
                ImportAction::AddInTreeList { common_segments: m, .. },
            ) => n > m,
            (
                ImportAction::AddInTreeList { common_segments: n, .. },
                ImportAction::AddNestedImport { common_segments: m, .. },
            ) => n > m,
            (ImportAction::AddInTreeList { .. }, _) => true,
            (ImportAction::AddNestedImport { .. }, ImportAction::Nothing) => false,
            (ImportAction::AddNestedImport { .. }, _) => true,
            (ImportAction::AddNewUse { .. }, _) => false,
        }
    }
}

// Find out the best ImportAction to import target path against current_use_tree.
// If current_use_tree has a nested import the function gets called recursively on every UseTree inside a UseTreeList.
fn walk_use_tree_for_best_action<'a>(
    current_path_segments: &mut Vec<&'a ast::PathSegment>, // buffer containing path segments
    current_parent_use_tree_list: Option<&'a ast::UseTreeList>, // will be Some value if we are in a nested import
    current_use_tree: &'a ast::UseTree, // the use tree we are currently examinating
    target: &[&'a ast::PathSegment],    // the path we want to import
) -> ImportAction<'a> {
    // We save the number of segments in the buffer so we can restore the correct segments
    // before returning. Recursive call will add segments so we need to delete them.
    let prev_len = current_path_segments.len();

    let tree_list = current_use_tree.use_tree_list();
    let alias = current_use_tree.alias();

    let path = match current_use_tree.path() {
        Some(path) => path,
        None => {
            // If the use item don't have a path, it means it's broken (syntax error)
            return ImportAction::add_new_use(
                current_use_tree
                    .syntax()
                    .ancestors()
                    .find_map(ast::UseItem::cast)
                    .map(AstNode::syntax),
                true,
            );
        }
    };

    // This can happen only if current_use_tree is a direct child of a UseItem
    if let Some(name) = alias.and_then(ast::NameOwner::name) {
        if compare_path_segment_with_name(target[0], name) {
            return ImportAction::Nothing;
        }
    }

    collect_path_segments_raw(current_path_segments, path);

    // We compare only the new segments added in the line just above.
    // The first prev_len segments were already compared in 'parent' recursive calls.
    let left = target.split_at(prev_len).1;
    let right = current_path_segments.split_at(prev_len).1;
    let common = compare_path_segments(left, right);
    let mut action = match common {
        0 => ImportAction::add_new_use(
            // e.g: target is std::fmt and we can have
            // use foo::bar
            // We add a brand new use statement
            current_use_tree.syntax().ancestors().find_map(ast::UseItem::cast).map(AstNode::syntax),
            true,
        ),
        common if common == left.len() && left.len() == right.len() => {
            // e.g: target is std::fmt and we can have
            // 1- use std::fmt;
            // 2- use std::fmt:{ ... }
            if let Some(list) = tree_list {
                // In case 2 we need to add self to the nested list
                // unless it's already there
                let has_self = list.use_trees().map(ast::UseTree::path).any(|p| {
                    p.and_then(ast::Path::segment)
                        .and_then(ast::PathSegment::kind)
                        .filter(|k| *k == ast::PathSegmentKind::SelfKw)
                        .is_some()
                });

                if has_self {
                    ImportAction::Nothing
                } else {
                    ImportAction::add_in_tree_list(current_path_segments.len(), list, true)
                }
            } else {
                // Case 1
                ImportAction::Nothing
            }
        }
        common if common != left.len() && left.len() == right.len() => {
            // e.g: target is std::fmt and we have
            // use std::io;
            // We need to split.
            let segments_to_split = current_path_segments.split_at(prev_len + common).1;
            ImportAction::add_nested_import(
                prev_len + common,
                path,
                Some(segments_to_split[0]),
                false,
            )
        }
        common if left.len() > right.len() => {
            // e.g: target is std::fmt and we can have
            // 1- use std;
            // 2- use std::{ ... };

            // fallback action
            let mut better_action = ImportAction::add_new_use(
                current_use_tree
                    .syntax()
                    .ancestors()
                    .find_map(ast::UseItem::cast)
                    .map(AstNode::syntax),
                true,
            );
            if let Some(list) = tree_list {
                // Case 2, check recursively if the path is already imported in the nested list
                for u in list.use_trees() {
                    let child_action =
                        walk_use_tree_for_best_action(current_path_segments, Some(list), u, target);
                    if child_action.is_better(&better_action) {
                        better_action = child_action;
                        if let ImportAction::Nothing = better_action {
                            return better_action;
                        }
                    }
                }
            } else {
                // Case 1, split
                better_action = ImportAction::add_nested_import(prev_len + common, path, None, true)
            }
            better_action
        }
        common if left.len() < right.len() => {
            // e.g: target is std::fmt and we can have
            // use std::fmt::Debug;
            let segments_to_split = current_path_segments.split_at(prev_len + common).1;
            ImportAction::add_nested_import(
                prev_len + common,
                path,
                Some(segments_to_split[0]),
                true,
            )
        }
        _ => unreachable!(),
    };

    // If we are inside a UseTreeList adding a use statement become adding to the existing
    // tree list.
    action = match (current_parent_use_tree_list, action) {
        (Some(use_tree_list), ImportAction::AddNewUse { .. }) => {
            ImportAction::add_in_tree_list(prev_len, use_tree_list, false)
        }
        (_, _) => action,
    };

    // We remove the segments added
    current_path_segments.truncate(prev_len);
    return action;
}

fn best_action_for_target<'b, 'a: 'b>(
    container: &'a SyntaxNode,
    path: &'a ast::Path,
    target: &'b [&'a ast::PathSegment],
) -> ImportAction<'a> {
    let mut storage = Vec::with_capacity(16); // this should be the only allocation
    let best_action = container
        .children()
        .filter_map(ast::UseItem::cast)
        .filter_map(ast::UseItem::use_tree)
        .map(|u| walk_use_tree_for_best_action(&mut storage, None, u, target))
        .fold(None, |best, a| {
            best.and_then(|best| Some(*ImportAction::better(&best, &a))).or(Some(a))
        });

    match best_action {
        Some(action) => return action,
        None => {
            // We have no action we no use item was found in container so we find
            // another item and we use it as anchor.
            // If there are not items, we choose the target path itself as anchor.
            let anchor = container
                .children()
                .find_map(ast::ModuleItem::cast)
                .map(AstNode::syntax)
                .or(Some(path.syntax()));

            return ImportAction::add_new_use(anchor, false);
        }
    }
}

fn make_assist(action: &ImportAction, target: &[&ast::PathSegment], edit: &mut AssistBuilder) {
    match action {
        ImportAction::AddNewUse { anchor, add_after_anchor } => {
            make_assist_add_new_use(anchor, *add_after_anchor, target, edit)
        }
        ImportAction::AddInTreeList { common_segments, tree_list, add_self } => {
            // We know that the fist n segments already exists in the use statement we want
            // to modify, so we want to add only the last target.len() - n segments.
            let segments_to_add = target.split_at(*common_segments).1;
            make_assist_add_in_tree_list(tree_list, segments_to_add, *add_self, edit)
        }
        ImportAction::AddNestedImport {
            common_segments,
            path_to_split,
            first_segment_to_split,
            add_self,
        } => {
            let segments_to_add = target.split_at(*common_segments).1;
            make_assist_add_nested_import(
                path_to_split,
                first_segment_to_split,
                segments_to_add,
                *add_self,
                edit,
            )
        }
        _ => {}
    }
}

fn make_assist_add_new_use(
    anchor: &Option<&SyntaxNode>,
    after: bool,
    target: &[&ast::PathSegment],
    edit: &mut AssistBuilder,
) {
    if let Some(anchor) = anchor {
        let indent = ra_fmt::leading_indent(anchor);
        let mut buf = String::new();
        if after {
            buf.push_str("\n");
            if let Some(spaces) = indent {
                buf.push_str(spaces);
            }
        }
        buf.push_str("use ");
        fmt_segments_raw(target, &mut buf);
        buf.push_str(";");
        if !after {
            buf.push_str("\n\n");
            if let Some(spaces) = indent {
                buf.push_str(spaces);
            }
        }
        let position = if after { anchor.range().end() } else { anchor.range().start() };
        edit.insert(position, buf);
    }
}

fn make_assist_add_in_tree_list(
    tree_list: &ast::UseTreeList,
    target: &[&ast::PathSegment],
    add_self: bool,
    edit: &mut AssistBuilder,
) {
    let last = tree_list.use_trees().last();
    if let Some(last) = last {
        let mut buf = String::new();
        let comma = last.syntax().siblings(Direction::Next).find(|n| n.kind() == COMMA);
        let offset = if let Some(comma) = comma {
            comma.range().end()
        } else {
            buf.push_str(",");
            last.syntax().range().end()
        };
        if add_self {
            buf.push_str(" self")
        } else {
            buf.push_str(" ");
        }
        fmt_segments_raw(target, &mut buf);
        edit.insert(offset, buf);
    } else {

    }
}

fn make_assist_add_nested_import(
    path: &ast::Path,
    first_segment_to_split: &Option<&ast::PathSegment>,
    target: &[&ast::PathSegment],
    add_self: bool,
    edit: &mut AssistBuilder,
) {
    let use_tree = path.syntax().ancestors().find_map(ast::UseTree::cast);
    if let Some(use_tree) = use_tree {
        let (start, add_colon_colon) = if let Some(first_segment_to_split) = first_segment_to_split
        {
            (first_segment_to_split.syntax().range().start(), false)
        } else {
            (use_tree.syntax().range().end(), true)
        };
        let end = use_tree.syntax().range().end();

        let mut buf = String::new();
        if add_colon_colon {
            buf.push_str("::");
        }
        buf.push_str("{ ");
        if add_self {
            buf.push_str("self, ");
        }
        fmt_segments_raw(target, &mut buf);
        if !target.is_empty() {
            buf.push_str(", ");
        }
        edit.insert(start, buf);
        edit.insert(end, "}");
    }
}

pub(crate) fn auto_import(ctx: AssistCtx) -> Option<Assist> {
    let node = ctx.covering_node();
    let current_file = node.ancestors().find_map(ast::SourceFile::cast)?;

    let path = node.ancestors().find_map(ast::Path::cast)?;
    // We don't want to mess with use statements
    if path.syntax().ancestors().find_map(ast::UseItem::cast).is_some() {
        return None;
    }

    let segments = collect_path_segments(path)?;
    if segments.len() < 2 {
        return None;
    }

    ctx.build(format!("import {} in the current file", fmt_segments(&segments)), |edit| {
        let action = best_action_for_target(current_file.syntax(), path, &segments);
        make_assist(&action, segments.as_slice(), edit);
        if let Some(last_segment) = path.segment() {
            // Here we are assuming the assist will provide a  correct use statement
            // so we can delete the path qualifier
            edit.delete(TextRange::from_to(
                path.syntax().range().start(),
                last_segment.syntax().range().start(),
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helpers::{ check_assist, check_assist_not_applicable };

    #[test]
    fn test_auto_import_file_add_use_no_anchor() {
        check_assist(
            auto_import,
            "
std::fmt::Debug<|>
    ",
            "
use std::fmt::Debug;

Debug<|>
    ",
        );
    }

    #[test]
    fn test_auto_import_file_add_use() {
        check_assist(
            auto_import,
            "
use stdx;

impl std::fmt::Debug<|> for Foo {
}
    ",
            "
use stdx;
use std::fmt::Debug;

impl Debug<|> for Foo {
}
    ",
        );
    }

    #[test]
    fn test_auto_import_file_add_use_other_anchor() {
        check_assist(
            auto_import,
            "
impl std::fmt::Debug<|> for Foo {
}
    ",
            "
use std::fmt::Debug;

impl Debug<|> for Foo {
}
    ",
        );
    }

    #[test]
    fn test_auto_import_file_add_use_other_anchor_indent() {
        check_assist(
            auto_import,
            "
    impl std::fmt::Debug<|> for Foo {
    }
    ",
            "
    use std::fmt::Debug;

    impl Debug<|> for Foo {
    }
    ",
        );
    }

    #[test]
    fn test_auto_import_file_split_different() {
        check_assist(
            auto_import,
            "
use std::fmt;

impl std::io<|> for Foo {
}
    ",
            "
use std::{ io, fmt};

impl io<|> for Foo {
}
    ",
        );
    }

    #[test]
    fn test_auto_import_file_split_self_for_use() {
        check_assist(
            auto_import,
            "
use std::fmt;

impl std::fmt::Debug<|> for Foo {
}
    ",
            "
use std::fmt::{ self, Debug, };

impl Debug<|> for Foo {
}
    ",
        );
    }

    #[test]
    fn test_auto_import_file_split_self_for_target() {
        check_assist(
            auto_import,
            "
use std::fmt::Debug;

impl std::fmt<|> for Foo {
}
    ",
            "
use std::fmt::{ self, Debug};

impl fmt<|> for Foo {
}
    ",
        );
    }

    #[test]
    fn test_auto_import_file_add_to_nested_self_nested() {
        check_assist(
            auto_import,
            "
use std::fmt::{Debug, nested::{Display}};

impl std::fmt::nested<|> for Foo {
}
",
            "
use std::fmt::{Debug, nested::{Display, self}};

impl nested<|> for Foo {
}
",
        );
    }

    #[test]
    fn test_auto_import_file_add_to_nested_self_already_included() {
        check_assist(
            auto_import,
            "
use std::fmt::{Debug, nested::{self, Display}};

impl std::fmt::nested<|> for Foo {
}
",
            "
use std::fmt::{Debug, nested::{self, Display}};

impl nested<|> for Foo {
}
",
        );
    }

    #[test]
    fn test_auto_import_file_add_to_nested_nested() {
        check_assist(
            auto_import,
            "
use std::fmt::{Debug, nested::{Display}};

impl std::fmt::nested::Debug<|> for Foo {
}
",
            "
use std::fmt::{Debug, nested::{Display, Debug}};

impl Debug<|> for Foo {
}
",
        );
    }

    #[test]
    fn test_auto_import_file_alias() {
        check_assist(
            auto_import,
            "
use std::fmt as foo;

impl foo::Debug<|> for Foo {
}
",
            "
use std::fmt as foo;

impl Debug<|> for Foo {
}
",
        );
    }

    #[test]
    fn test_auto_import_not_applicable_one_segment() {
        check_assist_not_applicable(
            auto_import,
            "
impl foo<|> for Foo {
}
",
        );
    }
}
