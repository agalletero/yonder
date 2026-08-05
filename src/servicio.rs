//! Supervisión periódica sin ventana abierta.
//!
//! §3.6 dice que la aplicación **no es un demonio**, y sigue sin serlo: aquí no
//! hay ningún proceso que se quede corriendo. Lo que hay es una orden de una
//! sola pasada —`yonder supervise`— que reconcilia lo que debería estar arriba
//! con lo que lo está, y sale. Quien la ejecuta cada dos minutos es systemd.
//!
//! La diferencia importa: si el supervisor fuera un demonio, matarlo dejaría los
//! túneles sin vigilancia y habría que arrancarlo a mano tras cada reinicio. Con
//! una unidad de tipo `oneshot` disparada por un temporizador, el estado real
//! sigue viviendo en los sockets de control y la reconciliación es idempotente:
//! da igual cuántas veces se ejecute.
//!
//! `KillMode=process` no es un detalle menor. Los maestros son hijos `ssh -f`
//! que **tienen que sobrevivir** al final de la pasada; con el modo por defecto
//! systemd mata el grupo de control entero al terminar y los túneles caen a los
//! segundos de nacer.

use std::path::PathBuf;

use crate::error::{Error, Resultado};
use crate::rutas;

/// Nombre base de las unidades de usuario.
const UNIDAD: &str = "yonder";

/// Cada cuánto reconcilia el temporizador.
const INTERVALO_POR_DEFECTO: &str = "2min";

/// Directorio de unidades de usuario de systemd.
fn directorio_unidades() -> Resultado<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|r| !r.as_os_str().is_empty())
        .unwrap_or(rutas::hogar()?.join(".config"));
    Ok(base.join("systemd/user"))
}

pub fn ruta_servicio() -> Resultado<PathBuf> {
    Ok(directorio_unidades()?.join(format!("{UNIDAD}.service")))
}

pub fn ruta_temporizador() -> Resultado<PathBuf> {
    Ok(directorio_unidades()?.join(format!("{UNIDAD}.timer")))
}

/// Ruta absoluta del ejecutable actual, para escribirla en la unidad.
fn ejecutable() -> Resultado<PathBuf> {
    std::env::current_exe()
        .map_err(Error::Io)?
        .canonicalize()
        .map_err(Error::Io)
}

/// Contenido de la unidad de servicio.
pub fn texto_servicio(binario: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Reconcilia los túneles SSH de «yonder»\n\
         Documentation=man:ssh_config(5)\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         # Los maestros son hijos «ssh -f» que TIENEN que sobrevivir al final\n\
         # de la pasada. Con el KillMode por defecto (control-group) systemd\n\
         # mata todo el grupo al acabar y los túneles caen a los segundos de\n\
         # nacer.\n\
         KillMode=process\n\
         # Entrecomillado: una ruta con espacios partiría la orden en dos.\n\
         ExecStart=\"{binario}\" supervise\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

/// Contenido de la unidad de temporizador.
pub fn texto_temporizador(intervalo: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Supervisa los túneles SSH cada {intervalo}\n\
         \n\
         [Timer]\n\
         OnBootSec=30\n\
         OnUnitActiveSec={intervalo}\n\
         # Recupera la pasada perdida si el equipo estaba suspendido.\n\
         Persistent=true\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n"
    )
}

/// Escribe las unidades y activa el temporizador.
pub fn instalar(intervalo: Option<&str>) -> Resultado<Vec<PathBuf>> {
    let intervalo = intervalo.unwrap_or(INTERVALO_POR_DEFECTO);
    let directorio = directorio_unidades()?;
    std::fs::create_dir_all(&directorio).map_err(|e| Error::escribiendo(&directorio, e))?;

    let binario = ejecutable()?;
    let servicio = ruta_servicio()?;
    let temporizador = ruta_temporizador()?;

    std::fs::write(&servicio, texto_servicio(&binario.display().to_string()))
        .map_err(|e| Error::escribiendo(&servicio, e))?;
    std::fs::write(&temporizador, texto_temporizador(intervalo))
        .map_err(|e| Error::escribiendo(&temporizador, e))?;

    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", &format!("{UNIDAD}.timer")])?;

    tracing::info!(
        intervalo = %intervalo,
        binario = %binario.display(),
        "temporizador de supervisión instalado"
    );
    Ok(vec![servicio, temporizador])
}

/// Para el temporizador y borra las unidades.
pub fn desinstalar() -> Resultado<()> {
    // Si no estaban activas, `systemctl` protesta y da igual: el objetivo es
    // que dejen de estarlo.
    let _ = systemctl(&["disable", "--now", &format!("{UNIDAD}.timer")]);
    for ruta in [ruta_temporizador()?, ruta_servicio()?] {
        if ruta.exists() {
            std::fs::remove_file(&ruta).map_err(|e| Error::escribiendo(&ruta, e))?;
        }
    }
    let _ = systemctl(&["daemon-reload"]);
    tracing::info!("temporizador de supervisión desinstalado");
    Ok(())
}

/// Estado del temporizador tal como lo cuenta systemd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EstadoServicio {
    /// Las unidades no están escritas.
    NoInstalado,
    /// Escritas pero el temporizador no está activo.
    Parado,
    /// Activo y reconciliando.
    Activo,
    /// No hay systemd de usuario en esta sesión.
    SinSystemd,
}

impl EstadoServicio {
    pub fn etiqueta(&self) -> &'static str {
        match self {
            EstadoServicio::NoInstalado => "no instalado",
            EstadoServicio::Parado => "instalado pero parado",
            EstadoServicio::Activo => "activo",
            EstadoServicio::SinSystemd => "no hay systemd de usuario",
        }
    }
}

pub fn estado() -> Resultado<EstadoServicio> {
    if !hay_systemd() {
        return Ok(EstadoServicio::SinSystemd);
    }
    if !ruta_temporizador()?.exists() {
        return Ok(EstadoServicio::NoInstalado);
    }
    let activo = crate::ssh::ejecutar(
        "systemctl",
        &[
            "--user".to_string(),
            "is-active".to_string(),
            format!("{UNIDAD}.timer"),
        ],
        std::time::Duration::from_secs(10),
        &crate::ssh::Entorno::terminal(),
    )
    .map(|salida| salida.estandar.trim() == "active")
    .unwrap_or(false);

    Ok(if activo {
        EstadoServicio::Activo
    } else {
        EstadoServicio::Parado
    })
}

/// Arranca el temporizador y hace una pasada inmediata.
pub fn arrancar() -> Resultado<()> {
    systemctl(&["start", &format!("{UNIDAD}.timer")])?;
    systemctl(&["start", &format!("{UNIDAD}.service")])?;
    Ok(())
}

/// Para el temporizador. Los túneles vivos siguen vivos.
pub fn parar() -> Resultado<()> {
    systemctl(&["stop", &format!("{UNIDAD}.timer")])
}

fn hay_systemd() -> bool {
    std::path::Path::new("/run/systemd/system").exists()
}

fn systemctl(argumentos: &[&str]) -> Resultado<()> {
    if !hay_systemd() {
        return Err(Error::DefinicionInvalida(
            "esta sesión no tiene systemd de usuario; instala el temporizador a mano".to_string(),
        ));
    }
    let mut todos = vec!["--user".to_string()];
    todos.extend(argumentos.iter().map(|a| a.to_string()));
    crate::ssh::ejecutar_exigiendo_exito(
        "systemctl",
        &todos,
        std::time::Duration::from_secs(30),
        &crate::ssh::Entorno::terminal(),
    )?;
    Ok(())
}

/// Consejo sobre `loginctl enable-linger`.
///
/// Sin él, systemd de usuario para todos los servicios al cerrar la sesión
/// gráfica, y el temporizador deja de reconciliar aunque el equipo siga
/// encendido.
pub fn aviso_linger() -> String {
    let usuario = std::env::var("USER").unwrap_or_else(|_| "$USER".to_string());
    format!(
        "Para que siga supervisando con la sesión cerrada:\n    loginctl enable-linger {usuario}"
    )
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn el_servicio_no_mata_a_sus_hijos_al_terminar() {
        // Sin KillMode=process systemd mata el grupo de control entero al
        // acabar la pasada y los maestros «ssh -f» caen a los segundos de
        // nacer. Es el fallo más fácil de cometer aquí.
        let texto = texto_servicio("/usr/local/bin/yonder");
        assert!(texto.contains("KillMode=process"));
        assert!(texto.contains("Type=oneshot"));
        // Entrecomillado: una ruta con espacios partiría la orden en dos.
        assert!(texto.contains("ExecStart=\"/usr/local/bin/yonder\" supervise"));
    }

    #[test]
    fn el_temporizador_recupera_la_pasada_perdida() {
        let texto = texto_temporizador("2min");
        assert!(texto.contains("OnUnitActiveSec=2min"));
        assert!(texto.contains("Persistent=true"));
        assert!(texto.contains("WantedBy=timers.target"));
    }

    #[test]
    fn el_intervalo_se_refleja_en_la_descripcion() {
        assert!(texto_temporizador("30s").contains("cada 30s"));
    }
}
