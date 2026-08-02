//! Ciclo de vida de la conexión maestra (§3.2 y §3.6).
//!
//! Se abre **una** conexión maestra por alias, desprendida con `-fN`, y sobre
//! ella se activan y cancelan reenvíos en caliente. El maestro sobrevive al
//! cierre de la ventana: la aplicación es un panel, no un demonio.
//!
//! Detalle que decide todo el diseño: el maestro se abre con
//! `ClearAllForwardings=yes`. Así arranca **sin ningún reenvío**, aunque el
//! bloque `Host` del fichero declare varios, y cada túnel se activa por
//! separado con `-O forward`. Los `LocalForward` del fichero siguen ahí para
//! que `ssh <alias>` desde una terminal funcione igual (criterio 7, §12); lo
//! que se evita es que abrir el maestro los levante todos de golpe.

use crate::error::Resultado;
use crate::modelo::Host;

use super::control::Control;
use super::{ejecutar, Entorno, Salida, ESPERA_CONEXION};

/// Ajustes de apertura del maestro.
#[derive(Debug, Clone)]
pub struct OpcionesMaestro {
    /// Segundos que `ssh` espera a establecer la conexión TCP.
    pub espera_conexion: u32,
    /// `ServerAliveInterval` (§3.3).
    pub intervalo_latido: u32,
    /// `ServerAliveCountMax` (§3.3).
    pub latidos_perdidos: u32,
    /// `BatchMode=yes` impide cualquier prompt. Útil para comprobaciones
    /// automáticas donde colgarse esperando una contraseña sería peor que fallar.
    pub sin_interaccion: bool,
}

impl Default for OpcionesMaestro {
    fn default() -> Self {
        OpcionesMaestro {
            espera_conexion: 15,
            intervalo_latido: 15,
            latidos_perdidos: 3,
            sin_interaccion: false,
        }
    }
}

/// La conexión maestra de un alias.
#[derive(Debug, Clone)]
pub struct Maestro {
    pub control: Control,
}

impl Maestro {
    pub fn de(alias: &str) -> Resultado<Maestro> {
        Ok(Maestro {
            control: Control::de(alias)?,
        })
    }

    /// PID del maestro si está vivo.
    pub fn pid(&self) -> Resultado<Option<u32>> {
        self.control.comprobar()
    }

    pub fn activo(&self) -> bool {
        matches!(self.control.comprobar(), Ok(Some(_)))
    }

    /// Abre el maestro si no lo estaba ya. Devuelve el PID.
    ///
    /// Es idempotente: llamarlo con el maestro ya abierto no reconecta ni
    /// reautentica, solo devuelve el PID que ya había.
    pub fn abrir(
        &self,
        host: &Host,
        opciones: &OpcionesMaestro,
        entorno: &Entorno,
    ) -> Resultado<u32> {
        if let Some(pid) = self.control.comprobar()? {
            tracing::debug!(alias = %self.control.alias, pid, "el maestro ya estaba abierto");
            return Ok(pid);
        }

        // Un socket que existe pero no responde es basura de un maestro muerto.
        // `ssh -M` se negaría a arrancar con él delante.
        if self.control.socket.exists() {
            tracing::info!(alias = %self.control.alias, "se retira un socket muerto antes de abrir");
            self.control.retirar_restos();
        }
        self.control.preparar()?;

        let salida = ejecutar(
            "ssh",
            &self.argumentos_de_apertura(host, opciones),
            ESPERA_CONEXION,
            entorno,
        )?;

        if !salida.correcta() {
            self.control.retirar_restos();
            return Err(error_de_apertura(&self.control.alias, &salida));
        }

        match self.control.comprobar()? {
            Some(pid) => {
                tracing::info!(alias = %self.control.alias, pid, "maestro abierto");
                Ok(pid)
            }
            None => {
                self.control.retirar_restos();
                Err(crate::Error::OrdenFallida {
                    orden: format!("ssh -M {}", self.control.alias),
                    codigo: 0,
                    salida: "el maestro terminó sin dejar socket de control".to_string(),
                })
            }
        }
    }

    fn argumentos_de_apertura(&self, host: &Host, opciones: &OpcionesMaestro) -> Vec<String> {
        let mut argumentos: Vec<String> = vec![
            // -M: esta conexión es el maestro. -f: al fondo tras autenticar.
            // -N: sin orden remota, solo el túnel.
            "-M".into(),
            "-f".into(),
            "-N".into(),
            "-S".into(),
            self.control.socket.display().to_string(),
        ];

        let mut opcion = |clave: &str, valor: String| {
            argumentos.push("-o".into());
            argumentos.push(format!("{clave}={valor}"));
        };

        // El maestro nace sin reenvíos; los pone `-O forward` uno a uno.
        opcion("ClearAllForwardings", "yes".into());
        // Si un reenvío no se puede abrir, que se sepa en vez de fingir.
        opcion("ExitOnForwardFailure", "yes".into());
        // Persiste tras la salida del cliente inicial: es lo que le permite
        // sobrevivir al cierre de la ventana (§3.6).
        opcion("ControlPersist", "yes".into());
        opcion("ConnectTimeout", opciones.espera_conexion.to_string());
        opcion("ServerAliveInterval", opciones.intervalo_latido.to_string());
        opcion("ServerAliveCountMax", opciones.latidos_perdidos.to_string());
        // Prohibido aceptar claves de host a ciegas (§5.2). La verificación la
        // hace la aplicación mostrando el fingerprint.
        opcion("StrictHostKeyChecking", "yes".into());
        if opciones.sin_interaccion {
            opcion("BatchMode", "yes".into());
        }

        argumentos.push(host.alias.clone());
        argumentos
    }

    /// Cierra el maestro. Todos sus reenvíos caen con él.
    pub fn cerrar(&self) -> Resultado<()> {
        self.control.cerrar()
    }
}

/// Convierte el fallo de apertura en un error con la causa concreta.
fn error_de_apertura(alias: &str, salida: &Salida) -> crate::Error {
    let diagnostico = salida.diagnostico();
    let minusculas = diagnostico.to_ascii_lowercase();

    if minusculas.contains("clave del host") || minusculas.contains("host key") {
        return crate::Error::OrdenFallida {
            orden: format!("ssh -M {alias}"),
            codigo: salida.codigo.unwrap_or(-1),
            salida: format!(
                "{diagnostico}\n    Verifica la clave del host desde la aplicación antes de conectar."
            ),
        };
    }
    crate::Error::OrdenFallida {
        orden: format!("ssh -M {alias}"),
        codigo: salida.codigo.unwrap_or(-1),
        salida: diagnostico,
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    fn host_de_prueba() -> Host {
        let mut host = Host::nuevo("preprod");
        host.hostname = Some("host.interno".into());
        host
    }

    #[test]
    fn la_apertura_limpia_los_reenvios_del_fichero() {
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        let maestro = Maestro::de("preprod").unwrap();
        let argumentos =
            maestro.argumentos_de_apertura(&host_de_prueba(), &OpcionesMaestro::default());

        // Sin esto, abrir el maestro levantaría de golpe todos los LocalForward
        // del bloque Host y se perdería el control por túnel.
        assert!(argumentos.contains(&"ClearAllForwardings=yes".to_string()));
        assert!(argumentos.contains(&"-M".to_string()));
        assert!(argumentos.contains(&"-f".to_string()));
        assert!(argumentos.contains(&"-N".to_string()));
        assert_eq!(argumentos.last().unwrap(), "preprod");
    }

    #[test]
    fn nunca_se_desactiva_la_verificacion_de_clave_de_host() {
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        let maestro = Maestro::de("preprod").unwrap();
        let argumentos =
            maestro.argumentos_de_apertura(&host_de_prueba(), &OpcionesMaestro::default());
        assert!(argumentos.contains(&"StrictHostKeyChecking=yes".to_string()));
        assert!(!argumentos
            .iter()
            .any(|a| a.contains("StrictHostKeyChecking=no")));
    }

    #[test]
    fn el_modo_sin_interaccion_anade_batchmode() {
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        let maestro = Maestro::de("preprod").unwrap();
        let opciones = OpcionesMaestro {
            sin_interaccion: true,
            ..Default::default()
        };
        let argumentos = maestro.argumentos_de_apertura(&host_de_prueba(), &opciones);
        assert!(argumentos.contains(&"BatchMode=yes".to_string()));
    }

    #[test]
    fn los_latidos_salen_de_las_opciones() {
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        let maestro = Maestro::de("preprod").unwrap();
        let opciones = OpcionesMaestro {
            intervalo_latido: 20,
            latidos_perdidos: 5,
            ..Default::default()
        };
        let argumentos = maestro.argumentos_de_apertura(&host_de_prueba(), &opciones);
        assert!(argumentos.contains(&"ServerAliveInterval=20".to_string()));
        assert!(argumentos.contains(&"ServerAliveCountMax=5".to_string()));
    }
}
