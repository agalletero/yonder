//! Verificación de la clave de host (§5.2).
//!
//! **Prohibido `StrictHostKeyChecking=no`.** Aceptar cualquier clave a ciegas
//! deja abierta la puerta a un intermediario exactamente en el momento en que
//! importa: la primera conexión, cuando todavía no hay nada con lo que comparar.
//!
//! El flujo correcto es: escanear, calcular el fingerprint SHA-256, **mostrarlo
//! y esperar confirmación**, y solo entonces escribir en `known_hosts`. El
//! cálculo del fingerprint lo hace `ssh-keygen`: no se implementa aquí ni un
//! resumen criptográfico (§2.2).

use std::io::Write;

use crate::error::{Error, Resultado};
use crate::rutas;

use super::{ejecutar, Entorno, ESPERA_CONSULTA};

/// Tipos de clave que se piden al escanear, en orden de preferencia.
const TIPOS: &str = "ed25519,ecdsa,rsa";

/// Una clave pública de host, tal como la devuelve `ssh-keyscan`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaveHost {
    /// Línea completa lista para `known_hosts`.
    pub linea: String,
    /// `ssh-ed25519`, `ssh-rsa`…
    pub tipo: String,
    /// Fingerprint en el formato que muestra OpenSSH: `SHA256:…`.
    pub fingerprint: String,
    /// Número de bits, tal como lo reporta `ssh-keygen -l`.
    pub bits: u32,
}

impl ClaveHost {
    /// Nombre corto del algoritmo para la interfaz: `ED25519`, `RSA`…
    pub fn algoritmo(&self) -> String {
        self.tipo
            .trim_start_matches("ssh-")
            .trim_start_matches("ecdsa-sha2-")
            .to_ascii_uppercase()
    }
}

/// Destino tal como se escribe en `known_hosts`.
///
/// El puerto solo aparece si no es el 22, y entonces el host va entre
/// corchetes. Equivocarse aquí hace que la entrada no se encuentre luego.
pub fn destino_known_hosts(host: &str, puerto: u16) -> String {
    if puerto == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{puerto}")
    }
}

/// `true` si ya hay una clave para ese host en `known_hosts`.
pub fn ya_conocido(host: &str, puerto: u16) -> Resultado<bool> {
    let salida = ejecutar(
        "ssh-keygen",
        &["-F".to_string(), destino_known_hosts(host, puerto)],
        ESPERA_CONSULTA,
        &Entorno::terminal(),
    )?;
    // `ssh-keygen -F` devuelve 0 si encuentra la entrada y 1 si no.
    Ok(salida.correcta() && !salida.estandar.trim().is_empty())
}

/// Escanea las claves públicas de un host.
///
/// Esto **no** verifica nada por sí solo: solo trae lo que el host dice ser. La
/// verificación la hace el humano comparando el fingerprint con el que le
/// consta por otro canal.
pub fn escanear(host: &str, puerto: u16) -> Resultado<Vec<ClaveHost>> {
    let argumentos = vec![
        "-T".to_string(),
        "10".to_string(),
        "-p".to_string(),
        puerto.to_string(),
        "-t".to_string(),
        TIPOS.to_string(),
        host.to_string(),
    ];
    let salida = ejecutar(
        "ssh-keyscan",
        &argumentos,
        ESPERA_CONSULTA,
        &Entorno::terminal(),
    )?;

    let lineas: Vec<String> = salida
        .estandar
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect();

    if lineas.is_empty() {
        return Err(Error::OrdenFallida {
            orden: format!("ssh-keyscan {host}"),
            codigo: salida.codigo.unwrap_or(-1),
            salida: if salida.error.trim().is_empty() {
                format!("«{host}» no devolvió ninguna clave de host")
            } else {
                salida.diagnostico()
            },
        });
    }

    calcular_fingerprints(&lineas)
}

/// Calcula el fingerprint de cada línea con `ssh-keygen -l`.
fn calcular_fingerprints(lineas: &[String]) -> Resultado<Vec<ClaveHost>> {
    let mut temporal = tempfile_en_memoria()?;
    for linea in lineas {
        writeln!(temporal, "{linea}").map_err(Error::Io)?;
    }
    temporal.flush().map_err(Error::Io)?;
    let ruta = temporal.path().to_path_buf();

    let salida = ejecutar(
        "ssh-keygen",
        &[
            "-l".to_string(),
            "-f".to_string(),
            ruta.display().to_string(),
        ],
        ESPERA_CONSULTA,
        &Entorno::terminal(),
    )?;

    if !salida.correcta() {
        return Err(Error::OrdenFallida {
            orden: "ssh-keygen -l".to_string(),
            codigo: salida.codigo.unwrap_or(-1),
            salida: salida.diagnostico(),
        });
    }

    // Cada línea es: «256 SHA256:xxxx host (ED25519)», en el mismo orden.
    let mut claves = Vec::new();
    for (indice, resumen) in salida
        .estandar
        .lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
    {
        let campos: Vec<&str> = resumen.split_whitespace().collect();
        if campos.len() < 2 {
            continue;
        }
        let linea = match lineas.get(indice) {
            Some(l) => l.clone(),
            None => continue,
        };
        let tipo = linea
            .split_whitespace()
            .nth(1)
            .unwrap_or("desconocido")
            .to_string();
        claves.push(ClaveHost {
            linea,
            tipo,
            fingerprint: campos[1].to_string(),
            bits: campos[0].parse().unwrap_or(0),
        });
    }
    Ok(claves)
}

fn tempfile_en_memoria() -> Resultado<tempfile::NamedTempFile> {
    tempfile::Builder::new()
        .prefix("yonder-keyscan-")
        .tempfile()
        .map_err(Error::Io)
}

/// Añade claves ya confirmadas por el usuario a `~/.ssh/known_hosts`.
///
/// Solo se debe llamar **después** de que alguien haya mirado el fingerprint y
/// haya dicho que sí.
pub fn aceptar(claves: &[ClaveHost]) -> Resultado<()> {
    if claves.is_empty() {
        return Ok(());
    }
    let ruta = rutas::known_hosts()?;
    if let Some(directorio) = ruta.parent() {
        rutas::asegurar_directorio_privado(directorio)?;
    }

    use std::os::unix::fs::OpenOptionsExt;
    let mut fichero = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&ruta)
        .map_err(|e| Error::escribiendo(&ruta, e))?;

    for clave in claves {
        writeln!(fichero, "{}", clave.linea).map_err(|e| Error::escribiendo(&ruta, e))?;
        tracing::info!(
            tipo = %clave.tipo,
            fingerprint = %clave.fingerprint,
            "clave de host aceptada por el usuario"
        );
    }
    Ok(())
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn el_destino_lleva_corchetes_solo_con_puerto_no_estandar() {
        assert_eq!(destino_known_hosts("host.example", 22), "host.example");
        assert_eq!(
            destino_known_hosts("host.example", 2222),
            "[host.example]:2222"
        );
    }

    #[test]
    fn calcula_fingerprints_reales_con_ssh_keygen() {
        // Se genera un par de claves de verdad y se comprueba que el
        // fingerprint que devolvemos coincide con el que da ssh-keygen. Así la
        // prueba no depende de ningún host externo.
        let directorio = tempfile::tempdir().unwrap();
        let clave = directorio.path().join("prueba");
        let generacion = std::process::Command::new("ssh-keygen")
            .args([
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "prueba",
                "-f",
                &clave.display().to_string(),
            ])
            .status();
        if !matches!(generacion, Ok(estado) if estado.success()) {
            eprintln!("[WARN] ssh-keygen no disponible: se omite la comprobación");
            return;
        }

        let publica = std::fs::read_to_string(clave.with_extension("pub")).unwrap();
        let campos: Vec<&str> = publica.split_whitespace().collect();
        let linea = format!("host.ejemplo {} {}", campos[0], campos[1]);

        let claves = calcular_fingerprints(&[linea]).unwrap();
        assert_eq!(claves.len(), 1);
        assert!(claves[0].fingerprint.starts_with("SHA256:"));
        assert_eq!(claves[0].tipo, "ssh-ed25519");
        assert_eq!(claves[0].algoritmo(), "ED25519");
        assert_eq!(claves[0].bits, 256);
    }

    #[test]
    fn el_algoritmo_se_muestra_legible() {
        let clave = ClaveHost {
            linea: String::new(),
            tipo: "ecdsa-sha2-nistp256".into(),
            fingerprint: "SHA256:x".into(),
            bits: 256,
        };
        assert_eq!(clave.algoritmo(), "NISTP256");
    }
}
