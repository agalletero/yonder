//! Socket de control de OpenSSH: la única fuente de verdad del estado (§2.4).
//!
//! No hay una tabla de «túneles activos» en ninguna base de datos. Si quieres
//! saber si un maestro está vivo, se lo preguntas a él con `-O check`. Al
//! arrancar, la aplicación escanea el directorio de sockets y reconstruye la
//! vista (§3.6); si un socket no responde, está huérfano y se borra. Sin esa
//! limpieza la lista acaba mintiendo, que es peor que no tener lista.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{Error, Resultado};
use crate::modelo::Reenvio;
use crate::rutas;

use super::{ejecutar, Entorno, ESPERA_CONTROL};

/// Extensión del fichero que guarda a qué alias pertenece un socket.
///
/// El nombre del socket es un hash y no se puede invertir, pero `-O check`
/// necesita el destino. Guardar el alias al lado evita adivinar y permite que
/// la limpieza de huérfanos sea exacta en vez de heurística.
const EXTENSION_ALIAS: &str = "alias";

/// Un socket de control asociado a un alias.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Control {
    pub alias: String,
    pub socket: PathBuf,
}

impl Control {
    pub fn de(alias: &str) -> Resultado<Control> {
        Ok(Control {
            alias: alias.to_string(),
            socket: rutas::socket_control(alias)?,
        })
    }

    fn ruta_alias(&self) -> PathBuf {
        self.socket.with_extension(EXTENSION_ALIAS)
    }

    /// Prepara el directorio y deja constancia del alias.
    pub fn preparar(&self) -> Resultado<()> {
        let directorio = self
            .socket
            .parent()
            .ok_or(Error::RutaIndeterminada("XDG_RUNTIME_DIR"))?;
        rutas::asegurar_directorio_privado(directorio)?;
        let ruta = self.ruta_alias();
        std::fs::write(&ruta, &self.alias).map_err(|e| Error::escribiendo(&ruta, e))?;
        Ok(())
    }

    /// Argumentos comunes a todas las órdenes de control.
    fn argumentos(&self, orden: &str, extra: &[String]) -> Vec<String> {
        let mut argumentos = vec![
            "-S".to_string(),
            self.socket.display().to_string(),
            "-O".to_string(),
            orden.to_string(),
        ];
        argumentos.extend_from_slice(extra);
        argumentos.push(self.alias.clone());
        argumentos
    }

    /// `ssh -O check`: PID del maestro si está vivo, `None` si no lo está.
    ///
    /// Un fallo aquí no es un error de la aplicación: significa «no hay
    /// maestro», que es una respuesta perfectamente válida.
    pub fn comprobar(&self) -> Resultado<Option<u32>> {
        if !self.socket.exists() {
            return Ok(None);
        }
        let salida = ejecutar(
            "ssh",
            &self.argumentos("check", &[]),
            ESPERA_CONTROL,
            &Entorno::terminal(),
        )?;
        if !salida.correcta() {
            return Ok(None);
        }
        // El mensaje es «Master running (pid=12345)».
        Ok(extraer_pid(&salida.error).or_else(|| extraer_pid(&salida.estandar)))
    }

    /// `true` si el socket existe y hay alguien escuchando al otro lado.
    ///
    /// Sirve para sockets cuyo alias ya no está en la configuración: no se les
    /// puede pasar un destino a `-O check`, pero sí se puede comprobar si el
    /// fichero está vivo o es basura.
    pub fn socket_vivo(socket: &Path) -> bool {
        match UnixStream::connect(socket) {
            Ok(flujo) => {
                let _ = flujo.set_read_timeout(Some(Duration::from_millis(200)));
                drop(flujo);
                true
            }
            Err(_) => false,
        }
    }

    /// `ssh -O forward -L …`: añade un reenvío sin reconectar (§3.2).
    pub fn activar_reenvio(&self, reenvio: &Reenvio, entorno: &Entorno) -> Resultado<()> {
        self.orden_de_reenvio("forward", reenvio, entorno)
    }

    /// `ssh -O cancel -L …`: quita un reenvío sin cerrar la conexión.
    pub fn cancelar_reenvio(&self, reenvio: &Reenvio, entorno: &Entorno) -> Resultado<()> {
        self.orden_de_reenvio("cancel", reenvio, entorno)
    }

    fn orden_de_reenvio(&self, orden: &str, reenvio: &Reenvio, entorno: &Entorno) -> Resultado<()> {
        let extra = vec![reenvio.tipo.bandera().to_string(), reenvio.argumento()];
        let salida = ejecutar(
            "ssh",
            &self.argumentos(orden, &extra),
            ESPERA_CONTROL,
            entorno,
        )?;
        if salida.correcta() {
            return Ok(());
        }
        let diagnostico = salida.diagnostico();
        if diagnostico.contains("no hay ninguna conexión maestra") {
            return Err(Error::MaestroInactivo(self.alias.clone()));
        }
        Err(Error::OrdenFallida {
            orden: format!("ssh -O {orden} {}", reenvio.argumento()),
            codigo: salida.codigo.unwrap_or(-1),
            salida: diagnostico,
        })
    }

    /// `ssh -O exit`: cierra el maestro y todos sus reenvíos.
    pub fn cerrar(&self) -> Resultado<()> {
        if self.socket.exists() {
            let salida = ejecutar(
                "ssh",
                &self.argumentos("exit", &[]),
                ESPERA_CONTROL,
                &Entorno::terminal(),
            )?;
            if !salida.correcta() {
                tracing::warn!(
                    alias = %self.alias,
                    "el maestro no aceptó el cierre limpio: {}",
                    salida.diagnostico()
                );
            }
        }
        self.retirar_restos();
        Ok(())
    }

    /// Borra el socket y su fichero de alias.
    pub fn retirar_restos(&self) {
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_file(self.ruta_alias());
    }
}

fn extraer_pid(texto: &str) -> Option<u32> {
    let inicio = texto.find("pid=")? + 4;
    let resto = &texto[inicio..];
    let fin = resto
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(resto.len());
    resto[..fin].parse().ok()
}

/// Todos los sockets de control que hay en el directorio de ejecución.
///
/// Es lo que permite reconstruir la vista al arrancar: cierras la ventana, los
/// túneles siguen; la vuelves a abrir y se reengancha (§3.6).
pub fn descubrir() -> Resultado<Vec<Control>> {
    let directorio = rutas::ejecucion()?;
    let entradas = match std::fs::read_dir(&directorio) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::leyendo(&directorio, e)),
    };

    let mut encontrados = Vec::new();
    for entrada in entradas.flatten() {
        let ruta = entrada.path();
        let nombre = match ruta.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !nombre.starts_with(rutas::PREFIJO_SOCKET) || nombre.contains('.') {
            continue;
        }
        let alias = std::fs::read_to_string(ruta.with_extension(EXTENSION_ALIAS))
            .map(|texto| texto.trim().to_string())
            .unwrap_or_default();
        encontrados.push(Control {
            alias,
            socket: ruta,
        });
    }
    encontrados.sort_by(|a, b| a.socket.cmp(&b.socket));
    Ok(encontrados)
}

/// Informe de la limpieza de sockets huérfanos.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Limpieza {
    /// Sockets con un maestro vivo detrás.
    pub vivos: Vec<String>,
    /// Sockets borrados por no responder.
    pub retirados: Vec<PathBuf>,
}

/// Limpieza obligatoria de §3.6.
///
/// Si `-O check` falla sobre un socket existente, el socket está huérfano:
/// se borra junto con su fichero de alias. Para los sockets cuyo alias ya no
/// se conoce se usa una conexión de prueba al socket, que distingue igual de
/// bien un maestro vivo de un resto abandonado.
pub fn limpiar_huerfanos() -> Resultado<Limpieza> {
    let mut informe = Limpieza::default();

    for control in descubrir()? {
        let vivo = if control.alias.is_empty() {
            Control::socket_vivo(&control.socket)
        } else {
            control.comprobar()?.is_some()
        };

        if vivo {
            informe.vivos.push(if control.alias.is_empty() {
                control.socket.display().to_string()
            } else {
                control.alias.clone()
            });
        } else {
            tracing::info!(
                socket = %control.socket.display(),
                alias = %control.alias,
                "socket huérfano: se retira"
            );
            control.retirar_restos();
            informe.retirados.push(control.socket);
        }
    }
    Ok(informe)
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn extrae_el_pid_del_mensaje_de_openssh() {
        assert_eq!(extraer_pid("Master running (pid=12345)\n"), Some(12345));
        assert_eq!(extraer_pid("pid=7"), Some(7));
        assert_eq!(extraer_pid("Control socket connect: No such file"), None);
    }

    #[test]
    fn un_socket_inexistente_no_esta_vivo() {
        let temporal = tempfile::tempdir().unwrap();
        assert!(!Control::socket_vivo(
            &temporal.path().join("ctl-inexistente")
        ));
    }

    #[test]
    fn un_socket_sin_nadie_escuchando_no_esta_vivo() {
        let temporal = tempfile::tempdir().unwrap();
        let ruta = temporal.path().join("ctl-abandonado");
        // Un fichero normal con nombre de socket: exactamente lo que deja un
        // maestro que ha muerto de forma sucia.
        std::fs::write(&ruta, b"").unwrap();
        assert!(!Control::socket_vivo(&ruta));
    }

    #[test]
    fn un_socket_con_alguien_escuchando_esta_vivo() {
        let temporal = tempfile::tempdir().unwrap();
        let ruta = temporal.path().join("ctl-vivo");
        let escucha = std::os::unix::net::UnixListener::bind(&ruta).unwrap();
        assert!(Control::socket_vivo(&ruta));
        drop(escucha);
    }

    #[test]
    fn comprobar_sin_socket_devuelve_ninguno() {
        let temporal = tempfile::tempdir().unwrap();
        let control = Control {
            alias: "cualquiera".into(),
            socket: temporal.path().join("ctl-no-existe"),
        };
        assert_eq!(control.comprobar().unwrap(), None);
    }

    #[test]
    fn los_argumentos_llevan_el_destino_al_final() {
        let control = Control {
            alias: "preprod".into(),
            socket: PathBuf::from("/run/user/1000/yonder/ctl-abc"),
        };
        let argumentos =
            control.argumentos("forward", &["-L".into(), "3000:localhost:3000".into()]);
        assert_eq!(
            argumentos,
            vec![
                "-S",
                "/run/user/1000/yonder/ctl-abc",
                "-O",
                "forward",
                "-L",
                "3000:localhost:3000",
                "preprod"
            ]
        );
    }
}
