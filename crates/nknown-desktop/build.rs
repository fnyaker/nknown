use cxx_qt_build::{CxxQtBuilder, QmlFile, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(
        QmlModule::new("org.nknown.app")
            .qml_files([
                "qml/Main.qml",
                "qml/SearchField.qml",
                "qml/MapView.qml",
                "qml/ResultCard.qml",
                "qml/Inspector.qml",
                "qml/KindGlyph.qml",
            ])
            .qml_file(QmlFile::from("qml/Theme.qml").singleton(true)),
    )
    .qt_module("Quick")
    .qt_module("QuickControls2")
    .files(["src/bridge.rs"])
    .build();
}
