use data_flow_analyzer::fs::SourceFile;
use data_flow_analyzer::lang::python::PythonFrontend;
use data_flow_analyzer::lang::LanguageFrontend;
use std::fs;

fn main() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.py");
    fs::write(&path, r#"class Outer:
    def method(self):
        class Inner:
            def m(self):
                return 1
"#).unwrap();
    let source = SourceFile { absolute_path: path, relative_path: "sample.py".to_string() };
    let cache = PythonFrontend::new().parse_files(&[source]).unwrap();
    for class in &cache.classes {
        println!("CLASS {}", class.qualified_name);
    }
    for function in &cache.functions {
        println!("FN {} kind={} class_id={:?} scope={}", function.qualified_name, function.kind, function.class_id, function.scope_id);
    }
    for scope in &cache.scopes {
        println!("SCOPE {} kind={} parent={:?} owner={}", scope.scope_id, scope.scope_kind, scope.parent_scope_id, scope.owner_id);
    }
}
