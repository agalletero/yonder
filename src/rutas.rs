//! Rutas y convenciones XDG (§6).
//!
//! Regla dura: **ninguna ruta absoluta codificada**. Todo sale de las
//! variables XDG o, en su defecto, de los valores por defecto de la
//! especificación. Las variables de entorno tienen prioridad para que las
//! pruebas puedan redirigir el árbol completo a un directorio temporal.

use std::path::{Path, PathBuf};

use crate::error::{Error, Resultado};

/// Nombre de la aplicación, usado como subdirectorio en todas las rutas XDG.
pub const APLICACION: &str = "yonder";

/// Fichero de configuración SSH propiedad **exclusiva** de la aplicación (§3.1).
pub const FICHERO_TUNELES: &str = "config.d/yonder.conf";

/// Línea que la aplicación inserta en `~/.ssh/config` la primera vez.
pub const LINEA_INCLUDE: &str = "Include ~/.ssh/config.d/yonder.conf";

fn variable(nombre: &str) -> Option<PathBuf> {
    std::env::var_os(nombre)
        .map(PathBuf::from)
        .filter(|r| !r.as_os_str().is_empty())
}

/// `$HOME`. Es la base de las rutas XDG, que sí se definen sobre la variable.
pub fn hogar() -> Resultado<PathBuf> {
    variable("HOME")
        .or_else(dirs::home_dir)
        .ok_or(Error::RutaIndeterminada("HOME"))
}

/// Directorio de inicio **tal como lo resuelve OpenSSH**.
///
/// Este es un detalle sutil y con consecuencias. `ssh` expande el `~` de
/// `~/.ssh/config` y de `Include` usando la entrada de `passwd` del usuario
/// efectivo, no `$HOME`. Si esta aplicación escribiera su fichero según la
/// variable de entorno y `ssh` leyera otro sitio, los túneles existirían para
/// la ventana y no para la terminal: se rompería el criterio de aceptación 7 de
/// §12 sin que nada avisara, porque los dos lados creerían tener razón.
///
/// Así que para todo lo que hay bajo `~/.ssh` se resuelve como lo hace `ssh`,
/// y si `$HOME` no coincide se registra un aviso en vez de elegir en silencio.
fn hogar_segun_ssh() -> Resultado<PathBuf> {
    static PASSWD: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

    let del_passwd = PASSWD.get_or_init(|| {
        use std::ffi::{CStr, OsStr};
        use std::os::unix::ffi::OsStrExt;

        // `getpwuid` devuelve un puntero a memoria estática y no es reentrante;
        // el `OnceLock` garantiza que solo se llama una vez en todo el proceso.
        let entrada = unsafe { libc::getpwuid(libc::geteuid()) };
        if entrada.is_null() {
            return None;
        }
        let directorio = unsafe { (*entrada).pw_dir };
        if directorio.is_null() {
            return None;
        }
        let bytes = unsafe { CStr::from_ptr(directorio) }.to_bytes();
        if bytes.is_empty() {
            None
        } else {
            Some(PathBuf::from(OsStr::from_bytes(bytes)))
        }
    });

    match del_passwd {
        Some(ruta) => {
            if let Some(entorno) = variable("HOME") {
                if &entorno != ruta {
                    tracing::warn!(
                        home = %entorno.display(),
                        passwd = %ruta.display(),
                        "«$HOME» no coincide con el directorio de passwd; se usa el de passwd \
                         porque es el que mira «ssh» al expandir «~»"
                    );
                }
            }
            Ok(ruta.clone())
        }
        None => hogar(),
    }
}

/// `~/.ssh`, resuelto como lo resuelve `ssh`.
pub fn directorio_ssh() -> Resultado<PathBuf> {
    Ok(hogar_segun_ssh()?.join(".ssh"))
}

/// `~/.ssh/config` — nunca se reescribe, salvo la línea `Include` (§3.1).
pub fn config_ssh_usuario() -> Resultado<PathBuf> {
    Ok(directorio_ssh()?.join("config"))
}

/// `~/.ssh/config.d/yonder.conf` — fichero propio.
pub fn config_tuneles() -> Resultado<PathBuf> {
    Ok(directorio_ssh()?.join(FICHERO_TUNELES))
}

/// `~/.ssh/known_hosts`.
pub fn known_hosts() -> Resultado<PathBuf> {
    Ok(directorio_ssh()?.join("known_hosts"))
}

/// `$XDG_CONFIG_HOME/yonder` (por defecto `~/.config/yonder`).
pub fn config_aplicacion() -> Resultado<PathBuf> {
    let base = variable("XDG_CONFIG_HOME").unwrap_or(hogar()?.join(".config"));
    Ok(base.join(APLICACION))
}

/// `$XDG_CONFIG_HOME/yonder/config.toml`.
pub fn preferencias() -> Resultado<PathBuf> {
    Ok(config_aplicacion()?.join("config.toml"))
}

/// `$XDG_DATA_HOME/yonder` (por defecto `~/.local/share/yonder`).
pub fn datos() -> Resultado<PathBuf> {
    let base = variable("XDG_DATA_HOME").unwrap_or(hogar()?.join(".local/share"));
    Ok(base.join(APLICACION))
}

/// `$XDG_DATA_HOME/yonder/estado.db`.
pub fn base_de_datos() -> Resultado<PathBuf> {
    Ok(datos()?.join("estado.db"))
}

/// `$XDG_STATE_HOME/yonder` (por defecto `~/.local/state/yonder`).
pub fn estado_volatil() -> Resultado<PathBuf> {
    let base = variable("XDG_STATE_HOME").unwrap_or(hogar()?.join(".local/state"));
    Ok(base.join(APLICACION))
}

/// `$XDG_STATE_HOME/yonder/yonder.log`.
pub fn registro() -> Resultado<PathBuf> {
    Ok(estado_volatil()?.join("yonder.log"))
}

/// Longitud máxima segura para la ruta de un socket de control.
///
/// `sun_path` de `sockaddr_un` son 108 bytes contando el NUL final, y OpenSSH
/// añade un sufijo aleatorio de 17 caracteres (`.XXXXXXXXXXXXXXXX`) mientras
/// negocia el maestro. Pasarse no da un error de la aplicación sino un
/// `unix_listener: path too long` de `ssh` que no le dice nada a nadie.
pub const MAXIMO_RUTA_SOCKET: usize = 108 - 1 - 17;

/// Longitud del nombre de fichero de un socket de control: `ctl-` + 16 hex.
const LONGITUD_NOMBRE_SOCKET: usize = 4 + 16;

/// `$XDG_RUNTIME_DIR/yonder`.
///
/// Es el directorio de los sockets de control. `$XDG_RUNTIME_DIR` se limpia al
/// cerrar sesión, que es exactamente el comportamiento deseado (§6).
///
/// Si esa ruta no deja sitio para el socket dentro del límite de `sun_path`
/// —pasa con `$XDG_RUNTIME_DIR` apuntando a un directorio profundo, típico en
/// entornos de prueba y contenedores— se cae a `/tmp/yonder-<uid>`, que es
/// corto por construcción. Vale más perder el borrado automático al cerrar
/// sesión que no poder abrir ni un túnel.
pub fn ejecucion() -> Resultado<PathBuf> {
    let corto = || -> PathBuf {
        let uid = unsafe { libc::getuid() };
        std::env::temp_dir().join(format!("{APLICACION}-{uid}"))
    };

    let preferido = match variable("XDG_RUNTIME_DIR") {
        Some(base) => base.join(APLICACION),
        None => return Ok(corto()),
    };

    if cabe_un_socket(&preferido) {
        return Ok(preferido);
    }

    let alternativo = corto();
    tracing::warn!(
        preferido = %preferido.display(),
        alternativo = %alternativo.display(),
        "la ruta de $XDG_RUNTIME_DIR no deja sitio para el socket de control \
         ({MAXIMO_RUTA_SOCKET} bytes); se usan sockets en el directorio temporal"
    );
    Ok(alternativo)
}

/// `true` si en ese directorio cabe la ruta completa de un socket de control.
pub fn cabe_un_socket(directorio: &Path) -> bool {
    directorio.as_os_str().len() + 1 + LONGITUD_NOMBRE_SOCKET <= MAXIMO_RUTA_SOCKET
}

/// `$XDG_RUNTIME_DIR/yonder/askpass.sock`.
pub fn socket_askpass() -> Resultado<PathBuf> {
    Ok(ejecucion()?.join("askpass.sock"))
}

/// Socket de control de un alias: `$XDG_RUNTIME_DIR/yonder/ctl-<hash>`.
///
/// El nombre se deriva de un SHA-256 truncado del alias en lugar del alias
/// literal por dos motivos: los alias pueden contener caracteres inválidos para
/// un nombre de fichero, y la ruta del socket de control de OpenSSH tiene un
/// límite duro de 108 bytes (`sun_path`). 16 caracteres hexadecimales dejan
/// margen de sobra.
pub fn socket_control(alias: &str) -> Resultado<PathBuf> {
    Ok(ejecucion()?.join(format!("ctl-{}", huella_alias(alias))))
}

/// Prefijo de los sockets de control, para el descubrimiento al arrancar (§3.6).
pub const PREFIJO_SOCKET: &str = "ctl-";

/// Huella corta y estable de un alias.
pub fn huella_alias(alias: &str) -> String {
    use sha2::{Digest, Sha256};
    let resumen = Sha256::digest(alias.as_bytes());
    resumen
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

/// Crea un directorio con permisos 0700 si no existe.
///
/// Todo lo que escribe la aplicación es privado del usuario: sockets de
/// control, socket del askpass, base de datos de estado.
pub fn asegurar_directorio_privado(ruta: &Path) -> Resultado<()> {
    use std::os::unix::fs::PermissionsExt;

    if !ruta.exists() {
        std::fs::create_dir_all(ruta).map_err(|e| Error::escribiendo(ruta, e))?;
    }
    let permisos = std::fs::Permissions::from_mode(0o700);
    std::fs::set_permissions(ruta, permisos).map_err(|e| Error::escribiendo(ruta, e))?;
    Ok(())
}

/// Expande el `~` de una ruta de `ssh_config` **como lo expande `ssh`**.
///
/// `shellexpand::tilde` usa `$HOME`, y `ssh` usa la entrada de `passwd`. En un
/// equipo normal coinciden y la diferencia no se nota nunca, pero cuando no
/// coinciden el fallo es de los que cuesta ver: `estado_include()` compara la
/// ruta expandida del `Include` con la del fichero propio, y si cada una sale
/// de un sitio distinto nunca coinciden. El síntoma es que la aplicación cree
/// que falta el `Include` y **lo añade otra vez en cada arranque**.
pub fn expandir(ruta: &str) -> PathBuf {
    let hogar = hogar_segun_ssh()
        .or_else(|_| hogar())
        .unwrap_or_else(|_| PathBuf::from("/"));

    match ruta.strip_prefix('~') {
        Some(resto) => {
            // Solo «~/…» y «~» pelado: «~otrousuario/…» lo resuelve `ssh` con
            // el passwd de ese otro usuario y aquí no se adivina.
            if resto.is_empty() {
                hogar
            } else if let Some(cola) = resto.strip_prefix('/') {
                hogar.join(cola)
            } else {
                PathBuf::from(ruta)
            }
        }
        None => PathBuf::from(ruta),
    }
}

/// Reduce una ruta absoluta a su forma con `~` para mostrarla en la interfaz.
///
/// Se prueban las dos formas de resolver el hogar porque las rutas de la
/// aplicación mezclan ambas: XDG usa `$HOME` y `~/.ssh` usa `passwd`.
pub fn abreviar(ruta: &Path) -> String {
    for candidato in [hogar_segun_ssh(), hogar()].into_iter().flatten() {
        if let Ok(resto) = ruta.strip_prefix(&candidato) {
            return format!("~/{}", resto.display());
        }
    }
    ruta.display().to_string()
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn el_include_expandido_coincide_con_el_fichero_propio() {
        // Este es el fallo que se coló: `expandir` usaba $HOME y
        // `config_tuneles()` la entrada de passwd. Cuando no coinciden, la
        // comparación de `estado_include()` falla siempre y la aplicación
        // añade la línea Include otra vez en cada arranque.
        std::env::set_var("HOME", "/un/home/que/no/es/el/de/passwd");

        let esperada = config_tuneles().unwrap();
        let expandida = expandir(LINEA_INCLUDE.trim_start_matches("Include").trim());
        assert_eq!(
            expandida, esperada,
            "la ruta del Include debe expandirse igual que la del fichero propio"
        );
    }

    #[test]
    fn expandir_respeta_lo_que_no_lleva_tilde() {
        assert_eq!(
            expandir("/ruta/absoluta/config"),
            Path::new("/ruta/absoluta/config")
        );
        assert_eq!(expandir("relativa.conf"), Path::new("relativa.conf"));
        // «~otro/…» lo resuelve ssh con el passwd de ese usuario: aquí no se
        // adivina, se deja tal cual.
        assert_eq!(expandir("~otro/cosa"), Path::new("~otro/cosa"));
    }

    #[test]
    fn la_huella_es_estable_y_corta() {
        let a = huella_alias("tunel-preprod");
        let b = huella_alias("tunel-preprod");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
        assert_ne!(a, huella_alias("tunel-produccion"));
    }

    #[test]
    fn la_ruta_del_socket_cabe_en_sun_path() {
        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        let ruta = socket_control("un-alias-razonablemente-largo").unwrap();
        assert!(
            ruta.as_os_str().len() <= MAXIMO_RUTA_SOCKET,
            "ruta demasiado larga ({} bytes): {ruta:?}",
            ruta.as_os_str().len()
        );
    }

    #[test]
    fn un_runtime_dir_profundo_cae_al_directorio_temporal() {
        // Pasa de verdad: contenedores y entornos de prueba con rutas largas.
        // Sin el respaldo, `ssh` fallaría con «unix_listener: path too long».
        let profundo = "/tmp/".to_string() + &"un-directorio-bastante-largo/".repeat(4);
        std::env::set_var("XDG_RUNTIME_DIR", &profundo);
        let directorio = ejecucion().unwrap();
        assert!(
            !directorio.starts_with(&profundo),
            "debería haberse caído al directorio temporal, pero devolvió {directorio:?}"
        );
        assert!(cabe_un_socket(&directorio));

        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
    }

    #[test]
    fn el_limite_deja_sitio_al_sufijo_de_openssh() {
        // OpenSSH añade «.XXXXXXXXXXXXXXXX» al negociar el maestro; si el
        // límite fuera 107 la ruta cabría al crearla y no al usarla.
        assert_eq!(MAXIMO_RUTA_SOCKET, 90);
        assert!(cabe_un_socket(Path::new("/run/user/1000/yonder")));
        assert!(!cabe_un_socket(Path::new(&"/x".repeat(60))));
    }
}
