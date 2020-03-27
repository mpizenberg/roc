use crate::expr::{fmt_expr, is_multiline_expr};
use crate::pattern::fmt_pattern;
use crate::spaces::{fmt_spaces, newline, INDENT};
use bumpalo::collections::String;
use roc_parse::ast::{Def, Expr};

pub fn fmt_def<'a>(buf: &mut String<'a>, def: &'a Def<'a>, indent: u16) {
    use roc_parse::ast::Def::*;

    match def {
        Annotation(_, _) => panic!("TODO have format_def support Annotation"),
        Alias { .. } => panic!("TODO have format_def support Alias"),
        Body(loc_pattern, loc_expr) => {
            fmt_pattern(buf, &loc_pattern.value, indent, true, false);
            buf.push_str(" =");
            if is_multiline_expr(&loc_expr.value) {
                match &loc_expr.value {
                    Expr::Record { .. } | Expr::List(_) => {
                        newline(buf, indent + INDENT);
                        fmt_expr(buf, &loc_expr.value, indent + INDENT, false, true);
                    }
                    _ => {
                        buf.push(' ');
                        fmt_expr(buf, &loc_expr.value, indent, false, true);
                    }
                }
            } else {
                buf.push(' ');
                fmt_expr(buf, &loc_expr.value, indent, false, true);
            }
        }
        TypedBody(_loc_pattern, _loc_annotation, _loc_expr) => {
            panic!("TODO support Annotation in TypedBody");
        }
        SpaceBefore(sub_def, spaces) => {
            fmt_spaces(buf, spaces.iter(), indent);
            fmt_def(buf, sub_def, indent);
        }
        SpaceAfter(sub_def, spaces) => {
            fmt_def(buf, sub_def, indent);

            fmt_spaces(buf, spaces.iter(), indent);
        }
        Nested(def) => fmt_def(buf, def, indent),
    }
}