use crate::model::{JavaRule, LoadStatement, ParsedBuildFile, RuleType};
use sha2::{Digest, Sha256};
use starlark_syntax::dialect::Dialect;
use starlark_syntax::syntax::ast::{ArgumentP, AstLiteral, AstNoPayload, CallArgsP, ExprP, StmtP};
use starlark_syntax::syntax::module::{AstModule, AstModuleFields};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Failed to parse BUILD file {path}: {message}")]
    SyntaxError { path: String, message: String },

    #[error("IO error reading BUILD file {path}: {error}")]
    IoError { path: String, error: String },
}

pub struct BuildFileParser;

impl BuildFileParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse_file(&self, path: &Path) -> Result<ParsedBuildFile, ParseError> {
        let content = std::fs::read_to_string(path).map_err(|e| ParseError::IoError {
            path: path.display().to_string(),
            error: e.to_string(),
        })?;

        let content_hash = Self::compute_hash(&content);
        let path_str = path.display().to_string();

        let module = match AstModule::parse(&path_str, content.clone(), &Dialect::Standard) {
            Ok(m) => m,
            Err(_) => {
                return Ok(ParsedBuildFile {
                    path: path.to_path_buf(),
                    content_hash,
                    rules: Vec::new(),
                    loads: Vec::new(),
                });
            }
        };

        let mut rules = Vec::new();
        let mut loads = Vec::new();
        let mut has_glob_or_var = false;

        Self::visit_statements(
            module.statement(),
            &mut rules,
            &mut loads,
            &mut has_glob_or_var,
        );

        Ok(ParsedBuildFile {
            path: path.to_path_buf(),
            content_hash,
            rules,
            loads,
        })
    }

    fn visit_statements(
        stmt: &starlark_syntax::syntax::ast::AstStmt,
        rules: &mut Vec<JavaRule>,
        loads: &mut Vec<LoadStatement>,
        has_glob_or_var: &mut bool,
    ) {
        match &stmt.node {
            StmtP::Expression(expr) => {
                if let ExprP::Call(func, args) = &expr.node {
                    Self::extract_rule_call(func, args, rules, has_glob_or_var);
                }
            }
            StmtP::Statements(stmts) => {
                for s in stmts {
                    Self::visit_statements(s, rules, loads, has_glob_or_var);
                }
            }
            StmtP::Assign(_) => {
                *has_glob_or_var = true;
            }
            StmtP::If { .. } => {
                *has_glob_or_var = true;
            }
            StmtP::For(for_stmt) => {
                *has_glob_or_var = true;
                Self::visit_statements(&for_stmt.body, rules, loads, has_glob_or_var);
            }
            StmtP::Def(def_stmt) => {
                *has_glob_or_var = true;
                Self::visit_statements(&def_stmt.body, rules, loads, has_glob_or_var);
            }
            StmtP::Load(load) => {
                loads.push(LoadStatement {
                    path: load.module.node.clone(),
                    symbols: load
                        .args
                        .iter()
                        .map(|a| a.local.node.ident.clone())
                        .collect(),
                });
            }
            _ => {}
        }
    }

    fn extract_rule_call(
        func: &starlark_syntax::syntax::ast::AstExpr,
        args: &CallArgsP<AstNoPayload>,
        rules: &mut Vec<JavaRule>,
        has_glob_or_var: &mut bool,
    ) {
        let rule_name = match &func.node {
            ExprP::Identifier(ident) => ident.node.ident.clone(),
            _ => return,
        };

        if !RuleType::is_java_rule(&rule_name) {
            return;
        }

        let mut name = String::new();
        let mut srcs = Vec::new();
        let mut deps = Vec::new();
        let mut runtime_deps = Vec::new();
        let mut resources = Vec::new();
        let mut plugins = Vec::new();
        let mut exports = Vec::new();
        let mut test_only = false;
        let mut visibility = Vec::new();

        for arg in &args.args {
            if let ArgumentP::Named(key, value) = &arg.node {
                match key.node.as_str() {
                    "name" => {
                        name = Self::extract_string(value);
                    }
                    "srcs" => {
                        srcs = Self::extract_string_list(value);
                        if Self::contains_glob_or_variable(value) {
                            *has_glob_or_var = true;
                        }
                    }
                    "deps" => {
                        deps = Self::extract_string_list(value);
                    }
                    "runtime_deps" => {
                        runtime_deps = Self::extract_string_list(value);
                    }
                    "resources" => {
                        resources = Self::extract_string_list(value);
                        if Self::contains_glob_or_variable(value) {
                            *has_glob_or_var = true;
                        }
                    }
                    "plugins" => {
                        plugins = Self::extract_string_list(value);
                    }
                    "exports" => {
                        exports = Self::extract_string_list(value);
                    }
                    "testonly" => {
                        test_only = Self::extract_bool(value);
                    }
                    "visibility" => {
                        visibility = Self::extract_string_list(value);
                    }
                    _ => {}
                }
            }
        }

        if !name.is_empty() {
            rules.push(JavaRule {
                rule_type: RuleType::from_rule_name(&rule_name),
                name,
                srcs,
                deps,
                runtime_deps,
                resources,
                plugins,
                exports,
                test_only,
                visibility,
            });
        }
    }

    fn extract_string(expr: &starlark_syntax::syntax::ast::AstExpr) -> String {
        match &expr.node {
            ExprP::Literal(AstLiteral::String(s)) => s.node.clone(),
            _ => String::new(),
        }
    }

    fn extract_bool(expr: &starlark_syntax::syntax::ast::AstExpr) -> bool {
        match &expr.node {
            ExprP::Identifier(ident) => ident.node.ident == "True",
            _ => false,
        }
    }

    fn extract_string_list(expr: &starlark_syntax::syntax::ast::AstExpr) -> Vec<String> {
        match &expr.node {
            ExprP::List(items) => items
                .iter()
                .filter_map(|item| match &item.node {
                    ExprP::Literal(AstLiteral::String(s)) => Some(s.node.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    fn contains_glob_or_variable(expr: &starlark_syntax::syntax::ast::AstExpr) -> bool {
        match &expr.node {
            ExprP::Call(func, _) => {
                if let ExprP::Identifier(ident) = &func.node {
                    return ident.node.ident == "glob";
                }
                false
            }
            ExprP::List(items) => items.iter().any(Self::contains_glob_or_variable),
            ExprP::Identifier(_) => true,
            _ => false,
        }
    }

    pub fn compute_hash(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

impl Default for BuildFileParser {
    fn default() -> Self {
        Self::new()
    }
}
