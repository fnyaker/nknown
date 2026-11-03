// Jetons de la charte (design/tokens.toml, docs/ui-guidelines.md) : une seule source pour l'app.
pragma Singleton
import QtQuick

QtObject {
    readonly property bool dark: Qt.styleHints.colorScheme !== Qt.ColorScheme.Light

    // Couleurs
    readonly property color bgBase: dark ? "#0A0D13" : "#F4F6FA"
    readonly property color bgRaised: dark ? "#11151E" : "#FFFFFF"
    readonly property color bgHover: dark ? "#161B26" : "#F7F9FC"
    readonly property color bgOverlay: dark ? "#E6171C28" : "#F0FFFFFF"
    readonly property color lineSubtle: dark ? "#0FFFFFFF" : "#140A0D13"
    readonly property color textPrimary: dark ? "#E8ECF4" : "#121722"
    readonly property color textSecondary: dark ? "#8D96A8" : "#5A6375"
    readonly property color accent: dark ? "#7CF3FF" : "#0091A8"
    readonly property color danger: dark ? "#FF6B81" : "#D63352"
    readonly property color ok: dark ? "#5CF0A6" : "#0E9F63"

    // Une teinte par nature de nœud.
    function kindColor(kind) {
        switch (kind) {
        case "query": return accent
        case "term": return dark ? "#6AA8FF" : "#2D6BE0"
        case "qualifier": return dark ? "#B58CFF" : "#7A4DE0"
        case "prose": case "related": return dark ? "#FFC46B" : "#C98200"
        case "code": return dark ? "#5CF0A6" : "#0E9F63"
        default: return dark ? "#8D96A8" : "#7A8394"
        }
    }
    function kindName(kind) {
        switch (kind) {
        case "query": return "requête"
        case "term": return "mot"
        case "qualifier": return "qualifiant"
        case "prose": return "texte"
        case "code": return "code"
        case "source": return "source"
        default: return "apparenté"
        }
    }

    // Typographie
    readonly property string fontUi: "Inter"
    readonly property string fontCode: "JetBrains Mono"

    // Mouvement (ms) ; le ressort vaut pour la carte et la caméra.
    readonly property int quick: 120
    readonly property int standard: 240
    readonly property int emphasis: 420
    readonly property int stagger: 30
    readonly property real springStiffness: 3.2
    readonly property real springDamping: 0.32
    readonly property real dim: 0.25

    // Formes
    readonly property int radiusCard: 10
    readonly property int radiusPanel: 14
    readonly property int margin: 24

    /// Date relative courte (« il y a 3 h »).
    function ago(seconds) {
        if (!seconds) return ""
        const d = Date.now() / 1000 - seconds
        if (d < 90) return "à l'instant"
        if (d < 3600) return "il y a " + Math.round(d / 60) + " min"
        if (d < 86400) return "il y a " + Math.round(d / 3600) + " h"
        if (d < 2 * 86400) return "hier"
        if (d < 30 * 86400) return "il y a " + Math.round(d / 86400) + " j"
        return new Date(seconds * 1000).toLocaleDateString(Qt.locale(), "d MMM yyyy")
    }

    /// Extrait en texte enrichi : mots de la requête en couleur de mot, sans gras ni fond.
    function highlight(text, words) {
        let html = text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
        const terms = (words || []).filter(w => w.length > 1)
            .map(w => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
        if (terms.length === 0) return html
        const re = new RegExp("(" + terms.join("|") + ")", "gi")
        return html.replace(re, "<font color=\"" + kindColor("term") + "\">$1</font>")
    }
}
