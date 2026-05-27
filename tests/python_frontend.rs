use data_flow_analyzer::fs::SourceFile;
use data_flow_analyzer::lang::LanguageFrontend;
use data_flow_analyzer::lang::python::PythonFrontend;
use std::fs;

#[test]
fn python_frontend_extracts_core_ir() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.py");
    fs::write(
        &path,
        r#"
from app.config import settings as cfg

class Child(Base):
    class_value = 1

    def method(self, x):
        y = x + self.class_value
        return y
"#,
    )
    .unwrap();

    let source = SourceFile {
        absolute_path: path,
        relative_path: "sample.py".to_string(),
    };

    let cache = PythonFrontend::new().parse_files(&[source]).unwrap();
    assert_eq!(cache.modules.len(), 1);
    assert!(cache.imports().iter().any(|i| i.module == "app.config"));
    assert!(cache.classes.iter().any(|c| c.qualified_name == "Child"));
    assert!(cache.functions.iter().any(|f| f.qualified_name == "Child.method"));
    assert!(cache.definitions.iter().any(|d| d.def_kind == "assign"));
    assert!(cache.uses.iter().any(|u| u.context.contains("return")));
}
