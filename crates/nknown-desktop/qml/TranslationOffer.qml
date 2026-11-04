// Proposition de traduction (accueil) : une source qui publie surtout dans une autre langue est
// proposée à la traduction en français. Une à la fois ; « Non merci » ne la repropose plus.
// Pendant la traduction des documents déjà indexés, la carte montre l'avancement.
import QtQuick

Rectangle {
    id: offer
    /// État de la traduction (TranslationView), relu avec l'état du moteur.
    property var translation: null
    signal answer(string spec, bool accept)

    readonly property var pending: translation && translation.available
        ? (translation.sources || []).filter(s => s.state === "suggested")
        : []
    readonly property var current: pending.length > 0 ? pending[0] : null
    readonly property var busy: translation ? translation.busy || null : null

    readonly property bool shown: current !== null || busy !== null
    implicitHeight: shown ? row.implicitHeight + 24 : 0
    radius: Theme.radiusCard
    color: Theme.bgRaised
    border.color: Theme.lineSubtle
    border.width: 1

    function langName(code) {
        return ({ eng: "anglais", deu: "allemand", spa: "espagnol", ita: "italien",
                  nld: "néerlandais", rus: "russe", zho: "chinois" })[code] || code
    }
    function short(spec) {
        // Dossier : ses deux derniers éléments ; adresse : sans le schéma.
        let s = spec
        if (s.startsWith("/")) {
            const parts = s.split("/").filter(p => p.length > 0)
            s = (parts.length > 2 ? "…/" : "/") + parts.slice(-2).join("/")
        } else {
            s = s.replace(/^(rss|web|wiki[a-z-]*):/, "").replace(/^https?:\/\//, "")
        }
        return s.length > 48 ? s.slice(0, 46) + "…" : s
    }

    Row {
        id: row
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        anchors.leftMargin: 16
        anchors.rightMargin: 12
        spacing: 12

        Text {
            id: label
            width: parent.width - (offer.busy ? 0 : actions.width + parent.spacing)
            anchors.verticalCenter: parent.verticalCenter
            wrapMode: Text.Wrap
            color: Theme.textPrimary
            font.family: Theme.fontUi
            font.pixelSize: 13
            textFormat: Text.StyledText
            text: {
                if (offer.busy)
                    return "Traduction de <b>" + offer.short(offer.busy.spec) + "</b> en français : "
                         + offer.busy.done + " / " + offer.busy.total
                const c = offer.current
                if (!c) return ""
                return "<b>" + offer.short(c.spec) + "</b> publie surtout en " + offer.langName(c.lang)
                     + " (" + Math.round(c.share * 100) + " % de " + c.docs + " documents). "
                     + "La traduire en français, sur cet ordinateur ?"
                     + (c.model_ready ? "" : "<br><font color='" + Theme.textSecondary
                        + "'>Modèle de traduction à télécharger (≈ 150 Mo, une fois).</font>")
            }
        }
        Row {
            id: actions
            visible: !offer.busy
            anchors.verticalCenter: parent.verticalCenter
            spacing: 8
            Pill {
                text: "Traduire"
                onClicked: if (offer.current) offer.answer(offer.current.spec, true)
            }
            Pill {
                text: "Non merci"
                quiet: true
                onClicked: if (offer.current) offer.answer(offer.current.spec, false)
            }
        }
    }
}
