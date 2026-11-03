//! Jeton du serveur distant dans KWallet (interface D-Bus `org.kde.KWallet` de kwalletd6) : jamais
//! en clair dans un fichier. Dossier « NKnown », une entrée par adresse de serveur.

use zbus::Connection;

const SERVICE: &str = "org.kde.kwalletd6";
const PATH: &str = "/modules/kwalletd6";
const IFACE: &str = "org.kde.KWallet";
const APP: &str = "NKnown";
const FOLDER: &str = "NKnown";

pub type Result<T> = std::result::Result<T, String>;

struct Wallet {
    conn: Connection,
    handle: i32,
}

fn call<B: serde::Serialize + zbus::zvariant::DynamicType>(
    conn: &Connection,
    method: &str,
    body: &B,
) -> Result<zbus::Message> {
    zbus::block_on(conn.call_method(Some(SERVICE), PATH, Some(IFACE), method, body))
        .map_err(|e| format!("KWallet : {e}"))
}

fn reply<T: serde::de::DeserializeOwned + zbus::zvariant::Type>(msg: zbus::Message) -> Result<T> {
    msg.body()
        .deserialize()
        .map_err(|e| format!("KWallet : {e}"))
}

impl Wallet {
    /// Ouvre le portefeuille réseau (KWallet peut demander son mot de passe à l'utilisateur).
    fn open() -> Result<Self> {
        let conn =
            zbus::block_on(Connection::session()).map_err(|e| format!("bus de session : {e}"))?;
        let wallet: String = reply(call(&conn, "networkWallet", &())?)?;
        let handle: i32 = reply(call(&conn, "open", &(wallet.as_str(), 0i64, APP))?)?;
        if handle < 0 {
            return Err("KWallet n'a pas pu être ouvert".into());
        }
        Ok(Self { conn, handle })
    }
}

impl Drop for Wallet {
    fn drop(&mut self) {
        let _ = call(&self.conn, "close", &(self.handle, false, APP));
    }
}

pub fn store_token(server: &str, token: &str) -> Result<()> {
    let w = Wallet::open()?;
    let has: bool = reply(call(&w.conn, "hasFolder", &(w.handle, FOLDER, APP))?)?;
    if !has {
        call(&w.conn, "createFolder", &(w.handle, FOLDER, APP))?;
    }
    let code: i32 = reply(call(
        &w.conn,
        "writePassword",
        &(w.handle, FOLDER, server, token, APP),
    )?)?;
    if code == 0 {
        Ok(())
    } else {
        Err(format!("KWallet : écriture refusée ({code})"))
    }
}

pub fn token(server: &str) -> Result<String> {
    let w = Wallet::open()?;
    let token: String = reply(call(
        &w.conn,
        "readPassword",
        &(w.handle, FOLDER, server, APP),
    )?)?;
    if token.is_empty() {
        Err("aucun jeton enregistré pour ce serveur".into())
    } else {
        Ok(token)
    }
}
