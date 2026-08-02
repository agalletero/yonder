//! Intercambio de claves con `ssh-copy-id` (§5.5).
//!
//! Se lanza como proceso hijo. La contraseña de esa primera —y con suerte
//! última— conexión la pide el askpass de §5.1 cuando se ejecuta desde la GUI,
//! o el TTY cuando se ejecuta desde la CLI. La aplicación no ve la clave
//! privada en ningún momento: solo pasa la ruta de la **pública**.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{Error, Resultado};
use crate::rutas;

use super::{ejecutar, Entorno};

/// `ssh-copy-id` puede tardar: hay que autenticarse una vez por el método viejo.
const ESPERA_COPIA: Duration = Duration::from_secs(120);

/// Nombres habituales de claves públicas, en orden de preferencia.
///
/// Las respaldadas por hardware van primero: si el usuario tiene una, es la que
/// quiere usar (§3.5).
const CANDIDATAS: &[&str] = &[
    "id_ed25519_sk.pub",
    "id_ecdsa_sk.pub",
    "id_ed25519.pub",
    "id_ecdsa.pub",
    "id_rsa.pub",
];

/// Claves públicas disponibles en `~/.ssh`, en orden de preferencia.
pub fn claves_publicas_disponibles() -> Resultado<Vec<PathBuf>> {
    let directorio = rutas::directorio_ssh()?;
    let mut encontradas: Vec<PathBuf> = CANDIDATAS
        .iter()
        .map(|nombre| directorio.join(nombre))
        .filter(|ruta| ruta.is_file())
        .collect();

    // Cualquier otra .pub que el usuario tenga, tras las conocidas.
    if let Ok(entradas) = std::fs::read_dir(&directorio) {
        let mut otras: Vec<PathBuf> = entradas
            .flatten()
            .map(|e| e.path())
            .filter(|ruta| ruta.extension().and_then(|e| e.to_str()) == Some("pub"))
            .filter(|ruta| !encontradas.contains(ruta))
            .collect();
        otras.sort();
        encontradas.extend(otras);
    }
    Ok(encontradas)
}

/// `true` si la clave está respaldada por hardware (FIDO2), por su tipo.
///
/// Detectarlo sirve para avisar en la interfaz de que habrá que tocar la llave
/// física. No requiere ningún código extra: el trabajo lo hace `ssh` (§3.5).
pub fn es_clave_hardware(ruta: &Path) -> bool {
    let contenido = match std::fs::read_to_string(ruta) {
        Ok(c) => c,
        Err(_) => {
            let nombre = ruta.file_name().and_then(|n| n.to_str()).unwrap_or("");
            return nombre.contains("_sk");
        }
    };
    contenido.starts_with("sk-")
}

/// Copia una clave pública al host mediante `ssh-copy-id`.
pub fn copiar_clave(alias: &str, clave_publica: &Path, entorno: &Entorno) -> Resultado<()> {
    if !clave_publica.is_file() {
        return Err(Error::DefinicionInvalida(format!(
            "«{}» no existe o no es un fichero",
            rutas::abreviar(clave_publica)
        )));
    }
    if clave_publica.extension().and_then(|e| e.to_str()) != Some("pub") {
        return Err(Error::DefinicionInvalida(
            "hay que indicar la clave **pública** (.pub); la privada no sale de aquí".to_string(),
        ));
    }

    let argumentos = vec![
        "-i".to_string(),
        clave_publica.display().to_string(),
        alias.to_string(),
    ];
    let salida = ejecutar("ssh-copy-id", &argumentos, ESPERA_COPIA, entorno)?;

    if salida.correcta() {
        tracing::info!(
            alias = %alias,
            clave = %rutas::abreviar(clave_publica),
            "clave pública instalada en el host"
        );
        return Ok(());
    }

    // `ssh-copy-id` sale con 0 y avisa por el flujo de error cuando la clave ya
    // estaba. Cualquier otro caso es un fallo real.
    Err(Error::OrdenFallida {
        orden: format!("ssh-copy-id -i {} {alias}", rutas::abreviar(clave_publica)),
        codigo: salida.codigo.unwrap_or(-1),
        salida: salida.diagnostico(),
    })
}

/// Comprueba que se puede entrar al host **solo con clave pública**.
///
/// Es la comprobación que permite ofrecer marcar el host como «solo clave
/// pública» y borrar del llavero la contraseña que hubiera (§5.5). `BatchMode`
/// impide cualquier prompt: si hiciera falta contraseña, falla en vez de
/// colgarse esperando.
pub fn verificar_solo_clave(alias: &str) -> Resultado<bool> {
    let argumentos = vec![
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "PreferredAuthentications=publickey".to_string(),
        "-o".to_string(),
        "ConnectTimeout=10".to_string(),
        alias.to_string(),
        "true".to_string(),
    ];
    let salida = ejecutar(
        "ssh",
        &argumentos,
        Duration::from_secs(30),
        &Entorno::terminal(),
    )?;
    Ok(salida.correcta())
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn rechaza_una_clave_privada() {
        let directorio = tempfile::tempdir().unwrap();
        let privada = directorio.path().join("id_ed25519");
        std::fs::write(&privada, "no importa").unwrap();
        let error = copiar_clave("host", &privada, &Entorno::terminal()).unwrap_err();
        assert!(error.to_string().contains("pública"));
    }

    #[test]
    fn rechaza_una_ruta_inexistente() {
        let error = copiar_clave(
            "host",
            Path::new("/no/existe/clave.pub"),
            &Entorno::terminal(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("no existe"));
    }

    #[test]
    fn detecta_una_clave_de_hardware_por_su_contenido() {
        let directorio = tempfile::tempdir().unwrap();
        let hardware = directorio.path().join("id_ed25519_sk.pub");
        std::fs::write(
            &hardware,
            "sk-ssh-ed25519@openssh.com AAAA... usuario@equipo\n",
        )
        .unwrap();
        assert!(es_clave_hardware(&hardware));

        let normal = directorio.path().join("id_ed25519.pub");
        std::fs::write(&normal, "ssh-ed25519 AAAA... usuario@equipo\n").unwrap();
        assert!(!es_clave_hardware(&normal));
    }
}
