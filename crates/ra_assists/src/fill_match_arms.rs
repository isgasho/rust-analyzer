use std::fmt::Write;

use hir::{
    AdtDef, Ty, FieldSource, source_binder,
};
use ra_syntax::ast::{self, AstNode};

use crate::{AssistCtx, Assist};

pub(crate) fn fill_match_arms(ctx: AssistCtx) -> Option<Assist> {
    let match_expr = ctx.node_at_offset::<ast::MatchExpr>()?;

    // We already have some match arms, so we don't provide any assists.
    match match_expr.match_arm_list() {
        Some(arm_list) if arm_list.arms().count() > 0 => {
            return None;
        }
        _ => {}
    }

    let expr = match_expr.expr()?;
    let function =
        source_binder::function_from_child_node(ctx.db, ctx.frange.file_id, expr.syntax())?;
    let infer_result = function.infer(ctx.db);
    let syntax_mapping = function.body_syntax_mapping(ctx.db);
    let node_expr = syntax_mapping.node_expr(expr)?;
    let match_expr_ty = infer_result[node_expr].clone();
    let enum_def = match match_expr_ty {
        Ty::Adt { def_id: AdtDef::Enum(e), .. } => e,
        _ => return None,
    };
    let enum_name = enum_def.name(ctx.db)?;
    let db = ctx.db;

    ctx.build("fill match arms", |edit| {
        let mut buf = format!("match {} {{\n", expr.syntax().text().to_string());
        let variants = enum_def.variants(db.as_ref());
        for variant in variants {
            let name = match variant.name(db.as_ref()) {
                Some(it) => it,
                None => continue,
            };
            write!(&mut buf, "    {}::{}", enum_name, name.to_string()).unwrap();

            let pat = variant
                .fields(db)
                .into_iter()
                .map(|field| {
                    let name = field.name(db).to_string();
                    let (_, source) = field.source(db);
                    match source {
                        FieldSource::Named(_) => name,
                        FieldSource::Pos(_) => "_".to_string(),
                    }
                })
                .collect::<Vec<_>>();

            match pat.first().map(|s| s.as_str()) {
                Some("_") => write!(&mut buf, "({})", pat.join(", ")).unwrap(),
                Some(_) => write!(&mut buf, "{{{}}}", pat.join(", ")).unwrap(),
                None => (),
            };

            buf.push_str(" => (),\n");
        }
        buf.push_str("}");
        edit.target(match_expr.syntax().range());
        edit.set_cursor(expr.syntax().range().start());
        edit.replace_node_and_indent(match_expr.syntax(), buf);
    })
}

#[cfg(test)]
mod tests {
    use crate::helpers::{check_assist, check_assist_target};

    use super::fill_match_arms;

    #[test]
    fn fill_match_arms_empty_body() {
        check_assist(
            fill_match_arms,
            r#"
            enum A {
                As,
                Bs,
                Cs(String),
                Ds(String, String),
                Es{x: usize, y: usize}
            }

            fn main() {
                let a = A::As;
                match a<|> {}
            }
            "#,
            r#"
            enum A {
                As,
                Bs,
                Cs(String),
                Ds(String, String),
                Es{x: usize, y: usize}
            }

            fn main() {
                let a = A::As;
                match <|>a {
                    A::As => (),
                    A::Bs => (),
                    A::Cs(_) => (),
                    A::Ds(_, _) => (),
                    A::Es{x, y} => (),
                }
            }
            "#,
        );
    }
    #[test]
    fn fill_match_arms_no_body() {
        check_assist(
            fill_match_arms,
            r#"
            enum E { X, Y}

            fn main() {
                match E::X<|>
            }
            "#,
            r#"
            enum E { X, Y}

            fn main() {
                match <|>E::X {
                    E::X => (),
                    E::Y => (),
                }
            }
            "#,
        );
    }

    #[test]
    fn fill_match_arms_target() {
        check_assist_target(
            fill_match_arms,
            r#"
            enum E { X, Y}

            fn main() {
                match E::X<|> {}
            }
            "#,
            "match E::X {}",
        );
    }
}
