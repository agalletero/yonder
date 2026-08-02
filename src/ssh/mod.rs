//! Envoltura del binario `ssh` del sistema (§3.2).
//!
//! Aquí no hay ni una línea de protocolo SSH. Todo lo que hace este módulo es
//! construir líneas de órdenes, ejecutarlas con un tiempo máximo y traducir
//! códigos de salida a errores en español. `ProxyJump` multisalto,
//! `known_hosts`, `ssh-agent`, claves FIDO2 y PKCS#11, GSSAPI y los algoritmos
//! los pone OpenSSH; nosotros no nos enteramos y esa es toda la gracia.

pub mod control;
pub mod copyid;
pub mod forward;
pub mod hostkey;
pub mod master;

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use wait_timeout::ChildExt;

use crate::error::{Error, Resultado};

pub use control::Control;
pub use master::{Maestro, OpcionesMaestro};

/// Tiempo máximo para las órdenes de control (`-O check`, `forward`, `cancel`).
///
/// Son instantáneas: hablan con un maestro que ya está conectado. Si tardan más
/// que esto, algo está colgado.
pub const ESPERA_CONTROL: Duration = Duration::from_secs(10);

/// Tiempo máximo para abrir un maestro.
///
/// Generoso a propósito: puede haber varios saltos, una llave FIDO2 que hay que
/// tocar (§3.5) o una contraseña que teclear.
pub const ESPERA_CONEXION: Duration = Duration::from_secs(120);

/// Tiempo máximo para consultas rápidas (`ssh -V`, `ssh-keyscan`).
pub const ESPERA_CONSULTA: Duration = Duration::from_secs(15);

/// Resultado de ejecutar un proceso externo.
#[derive(Debug, Clone)]
pub struct Salida {
    pub codigo: Option<i32>,
    pub estandar: String,
    pub error: String,
}

impl Salida {
    pub fn correcta(&self) -> bool {
        self.codigo == Some(0)
    }

    /// Mensaje más útil de los dos flujos, para mostrar al usuario.
    pub fn diagnostico(&self) -> String {
        let texto = if self.error.trim().is_empty() {
            self.estandar.trim()
        } else {
            self.error.trim()
        };
        traducir_error_de_ssh(texto)
    }
}

/// Entorno con el que se lanzan los procesos hijos.
///
/// Es donde se decide si las contraseñas las pide la terminal o el askpass
/// gráfico (§5.1). En la CLI se deja vacío y `ssh` usa el TTY, que es lo
/// correcto; en la GUI se apunta al binario auxiliar.
#[derive(Debug, Clone, Default)]
pub struct Entorno {
    /// Ruta a `yonder-askpass`. Si está presente se fuerza su uso.
    pub askpass: Option<PathBuf>,
    /// Variables adicionales.
    pub variables: BTreeMap<String, String>,
}

impl Entorno {
    /// Entorno de terminal: `ssh` preguntará por el TTY.
    pub fn terminal() -> Entorno {
        Entorno::default()
    }

    /// Entorno gráfico: las contraseñas las pide la aplicación (§5.1).
    pub fn grafico(askpass: PathBuf) -> Entorno {
        Entorno {
            askpass: Some(askpass),
            variables: BTreeMap::new(),
        }
    }

    fn aplicar(&self, orden: &mut Command) {
        for (clave, valor) in &self.variables {
            orden.env(clave, valor);
        }
        match &self.askpass {
            Some(ruta) => {
                orden.env("SSH_ASKPASS", ruta);
                // `force` exige OpenSSH >= 8.4 y es lo que hace que se use el
                // askpass aunque haya un TTY disponible. Sin esto, `ssh`
                // preferiría el terminal desde el que se lanzó la GUI.
                orden.env("SSH_ASKPASS_REQUIRE", "force");
                // Algunas versiones aún miran DISPLAY antes de usar el askpass.
                if std::env::var_os("DISPLAY").is_none() {
                    orden.env("DISPLAY", ":0");
                }
            }
            None => {
                // Que una variable heredada no secuestre el prompt del TTY.
                orden.env_remove("SSH_ASKPASS");
                orden.env_remove("SSH_ASKPASS_REQUIRE");
            }
        }
    }
}

/// Ejecuta un proceso externo con tiempo máximo (§11).
///
/// La salida se lee desde hilos aparte: si se dejara para después de esperar al
/// proceso, un hijo hablador llenaría la tubería y se bloquearía para siempre,
/// que es justo el cuelgue que el tiempo máximo pretende evitar.
pub fn ejecutar(
    programa: &str,
    argumentos: &[String],
    espera: Duration,
    entorno: &Entorno,
) -> Resultado<Salida> {
    let descripcion = format!("{programa} {}", argumentos.join(" "));
    tracing::debug!(orden = %descripcion, "ejecutando");

    let mut orden = Command::new(programa);
    orden
        .args(argumentos)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    entorno.aplicar(&mut orden);

    let mut hijo = match orden.spawn() {
        Ok(hijo) => hijo,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::EjecutableAusente(programa.to_string()))
        }
        Err(e) => return Err(Error::Io(e)),
    };

    let mut tuberia_salida = hijo.stdout.take();
    let mut tuberia_error = hijo.stderr.take();
    let lector_salida = std::thread::spawn(move || leer_todo(&mut tuberia_salida));
    let lector_error = std::thread::spawn(move || leer_todo(&mut tuberia_error));

    let estado = match hijo.wait_timeout(espera).map_err(Error::Io)? {
        Some(estado) => estado,
        None => {
            let _ = hijo.kill();
            let _ = hijo.wait();
            let _ = lector_salida.join();
            let _ = lector_error.join();
            return Err(Error::TiempoAgotado {
                orden: descripcion,
                segundos: espera.as_secs(),
            });
        }
    };

    let estandar = lector_salida.join().unwrap_or_default();
    let error = lector_error.join().unwrap_or_default();

    tracing::debug!(
        orden = %descripcion,
        codigo = ?estado.code(),
        "terminado"
    );

    Ok(Salida {
        codigo: estado.code(),
        estandar,
        error,
    })
}

/// Ejecuta y convierte un código distinto de cero en error.
pub fn ejecutar_exigiendo_exito(
    programa: &str,
    argumentos: &[String],
    espera: Duration,
    entorno: &Entorno,
) -> Resultado<Salida> {
    let salida = ejecutar(programa, argumentos, espera, entorno)?;
    if salida.correcta() {
        Ok(salida)
    } else {
        Err(Error::OrdenFallida {
            orden: format!("{programa} {}", argumentos.join(" ")),
            codigo: salida.codigo.unwrap_or(-1),
            salida: salida.diagnostico(),
        })
    }
}

fn leer_todo<L: Read>(tuberia: &mut Option<L>) -> String {
    let mut texto = String::new();
    if let Some(flujo) = tuberia.as_mut() {
        let mut bruto = Vec::new();
        let _ = flujo.read_to_end(&mut bruto);
        texto = String::from_utf8_lossy(&bruto).into_owned();
    }
    texto
}

/// Traduce los mensajes de `ssh` más frecuentes a algo accionable.
///
/// No se inventa nada: si el mensaje no está en la tabla, se devuelve tal cual.
/// El objetivo es que el usuario no vea un volcado críptico (§12, criterio 5).
fn traducir_error_de_ssh(texto: &str) -> String {
    let ultima = texto
        .lines()
        .filter(|l| !l.trim().is_empty())
        .rfind(|l| !l.starts_with("Warning: Permanently added"))
        .unwrap_or(texto)
        .trim();

    let minusculas = ultima.to_ascii_lowercase();
    let traduccion = if minusculas.contains("permission denied") {
        Some("autenticación rechazada: la clave o la contraseña no sirven para este usuario")
    } else if minusculas.contains("connection refused") {
        Some("el host rechazó la conexión: ¿está el servicio SSH escuchando en ese puerto?")
    } else if minusculas.contains("connection timed out")
        || minusculas.contains("operation timed out")
    {
        Some("el host no respondió a tiempo: ¿es alcanzable desde esta red?")
    } else if minusculas.contains("name or service not known")
        || minusculas.contains("could not resolve hostname")
    {
        Some("el nombre del host no se pudo resolver")
    } else if minusculas.contains("no route to host") {
        Some("no hay ruta hasta el host")
    } else if minusculas.contains("host key verification failed") {
        Some("la clave del host no está verificada: acéptala antes de conectar")
    } else if minusculas.contains("remote host identification has changed") {
        Some("la clave del host HA CAMBIADO: no se conecta hasta que lo revises a mano")
    } else if minusculas.contains("address already in use")
        || minusculas.contains("cannot listen to port")
    {
        Some("el puerto local ya está ocupado")
    } else if minusculas.contains("port forwarding failed") {
        Some("el reenvío no se pudo establecer")
    } else if minusculas.contains("control socket connect") {
        Some("no hay ninguna conexión maestra escuchando en ese socket")
    } else if minusculas.contains("too many authentication failures") {
        Some("demasiados intentos de autenticación: el agente está ofreciendo claves de más")
    } else {
        None
    };

    match traduccion {
        Some(claro) if claro != ultima => format!("{claro}\n    ssh dijo: {ultima}"),
        _ => ultima.to_string(),
    }
}

/// Versión de OpenSSH instalada, tal como la reporta `ssh -V`.
pub fn version_openssh() -> Resultado<String> {
    let salida = ejecutar(
        "ssh",
        &["-V".to_string()],
        ESPERA_CONSULTA,
        &Entorno::terminal(),
    )?;
    // `ssh -V` escribe en el flujo de error, no en el estándar.
    let texto = if salida.error.trim().is_empty() {
        salida.estandar
    } else {
        salida.error
    };
    Ok(texto.trim().to_string())
}

/// `true` si la versión instalada admite `SSH_ASKPASS_REQUIRE` (OpenSSH >= 8.4).
pub fn admite_askpass_forzado(version: &str) -> bool {
    let numero = version
        .split_whitespace()
        .find_map(|t| t.strip_prefix("OpenSSH_"))
        .unwrap_or("");
    let mut partes = numero.split(['.', 'p']);
    let mayor: u32 = partes.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let menor: u32 = partes.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (mayor, menor) >= (8, 4)
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn detecta_la_version_minima_para_el_askpass_forzado() {
        assert!(admite_askpass_forzado(
            "OpenSSH_10.2p1 Ubuntu-2ubuntu3.5, OpenSSL 3.5.5"
        ));
        assert!(admite_askpass_forzado("OpenSSH_8.4p1"));
        assert!(admite_askpass_forzado("OpenSSH_9.0p1, OpenSSL 3.0"));
        assert!(!admite_askpass_forzado("OpenSSH_8.2p1 Ubuntu-4ubuntu0.5"));
        assert!(!admite_askpass_forzado("cualquier cosa"));
    }

    #[test]
    fn traduce_los_fallos_habituales() {
        let texto = traducir_error_de_ssh("usuario@host: Permission denied (publickey,password).");
        assert!(texto.contains("autenticación rechazada"));
        assert!(texto.contains("ssh dijo:"));
    }

    #[test]
    fn deja_intacto_lo_que_no_conoce() {
        let texto = traducir_error_de_ssh("algo raro pasó");
        assert_eq!(texto, "algo raro pasó");
    }

    #[test]
    fn se_queda_con_la_ultima_linea_util() {
        let texto = traducir_error_de_ssh(
            "Warning: Permanently added 'host' to the list of known hosts.\nConnection refused",
        );
        assert!(texto.contains("rechazó la conexión"));
    }

    #[test]
    fn el_tiempo_maximo_mata_un_proceso_colgado() {
        let error = ejecutar(
            "sleep",
            &["30".to_string()],
            Duration::from_millis(200),
            &Entorno::terminal(),
        )
        .unwrap_err();
        assert!(matches!(error, Error::TiempoAgotado { .. }));
    }

    #[test]
    fn captura_las_dos_salidas() {
        let salida = ejecutar(
            "sh",
            &["-c".to_string(), "echo hola; echo adios >&2".to_string()],
            Duration::from_secs(5),
            &Entorno::terminal(),
        )
        .unwrap();
        assert!(salida.correcta());
        assert_eq!(salida.estandar.trim(), "hola");
        assert_eq!(salida.error.trim(), "adios");
    }

    #[test]
    fn un_ejecutable_que_no_existe_se_reporta_como_tal() {
        let error = ejecutar(
            "no-existe-este-binario-12345",
            &[],
            Duration::from_secs(1),
            &Entorno::terminal(),
        )
        .unwrap_err();
        assert!(matches!(error, Error::EjecutableAusente(_)));
    }
}
