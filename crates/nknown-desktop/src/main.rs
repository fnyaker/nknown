//! `nknown-app` : l'app native de NKnown (Qt6/QML via cxx-qt).
//!
//! Vues et règles visuelles : `docs/ui-guidelines.md`. Le moteur est le démon local ou, s'il
//! est arrêté, le cœur embarqué (`client.rs`).

mod bridge;
mod client;
mod instance;
mod secrets;
mod settings;
mod tray;
mod view;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QString, QUrl};

fn main() {
    // Une seule fenêtre : si l'app est déjà ouverte, elle reçoit la demande.
    let request = instance::Request::from_args(std::env::args().skip(1));
    if instance::forward(&request) {
        return;
    }
    let mut app = QGuiApplication::new();
    QGuiApplication::set_desktop_file_name(&QString::from("nknown"));
    if let Some(mut app) = app.as_mut() {
        app.as_mut().set_application_name(&QString::from("NKnown"));
        app.set_organization_domain(&QString::from("nknown.org"));
    }
    let mut engine = QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/org/nknown/app/qml/Main.qml"));
    }
    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
