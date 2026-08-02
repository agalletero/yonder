//! Protocolo entre `yonder-askpass` y la aplicación (§5.1).
//!
//! Una línea JSON de ida y otra de vuelta sobre un socket Unix. Deliberadamente
//! aburrido: el binario auxiliar tiene que ser trivial de leer y de auditar,
//! porque por él pasan contraseñas.
//!
//! Lo que **no** hace este protocolo, y es a propósito:
//!
//! - No cifra nada. El socket es 0600 en un directorio 0700 dentro de
//!   `$XDG_RUNTIME_DIR`: solo el propio usuario llega. Cifrarlo protegería de
//!   quien ya podría leer la memoria del proceso.
//! - No guarda nada. La respuesta viaja de la ventana a `ssh` y se acaba.

use serde::{Deserialize, Serialize};

/// Versión del protocolo. Si algún día cambia, el servidor podrá rechazar
/// clientes viejos en vez de malinterpretarlos.
pub const VERSION: u32 = 1;

/// Tiempo máximo que el binario auxiliar espera una respuesta.
pub const ESPERA_RESPUESTA_SEGUNDOS: u64 = 180;

/// Qué está pidiendo `ssh`.
///
/// Se deduce del texto del prompt. Sirve para que la ventana enseñe el modal
/// adecuado: no es lo mismo pedir una contraseña que avisar de que hay que
/// tocar una llave física.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tipo {
    /// Contraseña de la cuenta en el host remoto.
    Contrasena,
    /// Passphrase de una clave privada. La custodia `ssh-agent`, no nosotros.
    Passphrase,
    /// Hay que tocar la llave FIDO2 (§3.5). No se teclea nada.
    PresenciaFisica,
    /// PIN de la llave FIDO2.
    PinLlave,
    /// Pregunta de sí o no.
    Confirmacion,
    Desconocido,
}

impl Tipo {
    /// Clasifica el prompt de `ssh` por su texto.
    pub fn deducir(prompt: &str) -> Tipo {
        let texto = prompt.to_ascii_lowercase();
        if texto.contains("confirm user presence") || texto.contains("user presence") {
            Tipo::PresenciaFisica
        } else if texto.contains("pin for") || texto.contains("enter pin") {
            Tipo::PinLlave
        } else if texto.contains("passphrase") {
            Tipo::Passphrase
        } else if texto.contains("(yes/no") || texto.contains("[y/n]") {
            Tipo::Confirmacion
        } else if texto.contains("password") {
            Tipo::Contrasena
        } else {
            Tipo::Desconocido
        }
    }

    /// `true` si lo que se escribe debe ocultarse.
    pub fn es_secreto(&self) -> bool {
        matches!(self, Tipo::Contrasena | Tipo::Passphrase | Tipo::PinLlave)
    }

    /// `true` si no hay nada que teclear: solo se espera al usuario.
    pub fn sin_entrada(&self) -> bool {
        matches!(self, Tipo::PresenciaFisica)
    }

    /// Título para el modal.
    pub fn titulo(&self) -> &'static str {
        match self {
            Tipo::Contrasena => "Contraseña del host",
            Tipo::Passphrase => "Passphrase de la clave",
            Tipo::PresenciaFisica => "Toca la llave física",
            Tipo::PinLlave => "PIN de la llave",
            Tipo::Confirmacion => "Confirmación",
            Tipo::Desconocido => "SSH necesita una respuesta",
        }
    }
}

/// Lo que el binario auxiliar envía a la aplicación.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peticion {
    pub version: u32,
    /// Texto literal con el que `ssh` llamó al askpass.
    pub prompt: String,
    pub tipo: Tipo,
    /// PID del proceso que pregunta, para poder relacionarlo con un túnel.
    pub pid: u32,
}

impl Peticion {
    pub fn nueva(prompt: impl Into<String>) -> Peticion {
        let prompt = prompt.into();
        Peticion {
            version: VERSION,
            tipo: Tipo::deducir(&prompt),
            prompt,
            pid: std::process::id(),
        }
    }
}

/// Lo que la aplicación devuelve.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Respuesta {
    /// `None` si el usuario canceló.
    pub texto: Option<String>,
}

impl Respuesta {
    pub fn con(texto: impl Into<String>) -> Respuesta {
        Respuesta {
            texto: Some(texto.into()),
        }
    }

    pub fn cancelada() -> Respuesta {
        Respuesta { texto: None }
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn clasifica_los_prompts_reales_de_openssh() {
        assert_eq!(
            Tipo::deducir("usuario@host.example's password: "),
            Tipo::Contrasena
        );
        assert_eq!(
            Tipo::deducir("Enter passphrase for key '/home/usuario/.ssh/id_ed25519': "),
            Tipo::Passphrase
        );
        assert_eq!(
            Tipo::deducir("Confirm user presence for key ED25519-SK SHA256:abc"),
            Tipo::PresenciaFisica
        );
        assert_eq!(
            Tipo::deducir("Enter PIN for ED25519-SK key /home/usuario/.ssh/id_ed25519_sk: "),
            Tipo::PinLlave
        );
        assert_eq!(
            Tipo::deducir("Are you sure you want to continue connecting (yes/no/[fingerprint])?"),
            Tipo::Confirmacion
        );
    }

    #[test]
    fn la_presencia_fisica_no_pide_texto() {
        assert!(Tipo::PresenciaFisica.sin_entrada());
        assert!(!Tipo::PresenciaFisica.es_secreto());
        assert!(Tipo::Contrasena.es_secreto());
        assert!(!Tipo::Confirmacion.es_secreto());
    }

    #[test]
    fn la_peticion_va_y_vuelve_por_json() {
        let peticion = Peticion::nueva("usuario@host's password: ");
        let linea = serde_json::to_string(&peticion).unwrap();
        assert!(!linea.contains('\n'), "el protocolo es de una línea");
        let vuelta: Peticion = serde_json::from_str(&linea).unwrap();
        assert_eq!(vuelta.tipo, Tipo::Contrasena);
        assert_eq!(vuelta.version, VERSION);
    }

    #[test]
    fn la_cancelacion_se_distingue_de_la_cadena_vacia() {
        // Una contraseña vacía es una respuesta válida; cancelar no lo es.
        let vacia: Respuesta =
            serde_json::from_str(&serde_json::to_string(&Respuesta::con("")).unwrap()).unwrap();
        assert_eq!(vacia.texto.as_deref(), Some(""));

        let cancelada: Respuesta =
            serde_json::from_str(&serde_json::to_string(&Respuesta::cancelada()).unwrap()).unwrap();
        assert!(cancelada.texto.is_none());
    }
}
