//! Secretos en el Secret Service del sistema (§3.4).
//!
//! Tres decisiones que conviene tener presentes al leer este módulo:
//!
//! 1. **No hay password maestro propio.** GNOME Keyring y KWallet ya se
//!    desbloquean con la sesión. Añadir otro protegería lo mismo pidiendo el
//!    doble.
//! 2. **En el caso normal aquí no se guarda nada.** Con autenticación por clave
//!    pública, la passphrase la custodia `ssh-agent` y la aplicación no ve —ni
//!    quiere ver— ninguna clave privada.
//! 3. Lo único que puede llegar al llavero son contraseñas de hosts que aún
//!    usen autenticación por password. La interfaz las marca como caso a
//!    extinguir, y tras un `ssh-copy-id` correcto se ofrece borrarlas (§5.5).
//!
//! `oo7` habla Secret Service por D-Bus y también el portal en Flatpak, así que
//! sirve para las dos formas de distribuir la aplicación.

use std::collections::HashMap;

use crate::error::{Error, Resultado};

/// Valor del atributo que identifica lo que guarda esta aplicación.
const APLICACION: &str = "yonder";

fn traducir(e: oo7::Error) -> Error {
    Error::Secretos(e.to_string())
}

/// Acceso al llavero de la sesión.
///
/// `oo7` es asíncrono porque D-Bus lo es. Guardar una contraseña es una
/// operación excepcional en esta aplicación, así que en vez de teñir de `async`
/// todo el motor se levanta un ejecutor de un solo hilo aquí dentro.
pub struct Llavero {
    ejecutor: tokio::runtime::Runtime,
}

impl std::fmt::Debug for Llavero {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Llavero").finish_non_exhaustive()
    }
}

impl Llavero {
    pub fn abrir() -> Resultado<Llavero> {
        let ejecutor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Secretos(format!("no se pudo crear el ejecutor: {e}")))?;
        Ok(Llavero { ejecutor })
    }

    /// `true` si hay un Secret Service accesible en esta sesión.
    ///
    /// Sin llavero la aplicación sigue funcionando entera: solo deja de poder
    /// recordar contraseñas, que es el caso que se quiere extinguir de todas
    /// formas.
    pub fn disponible(&self) -> bool {
        self.ejecutor
            .block_on(async { oo7::Keyring::new().await.is_ok() })
    }

    fn atributos(alias: &str, usuario: &str) -> HashMap<&'static str, String> {
        let mut atributos = HashMap::new();
        atributos.insert("aplicacion", APLICACION.to_string());
        atributos.insert("alias", alias.to_string());
        atributos.insert("usuario", usuario.to_string());
        atributos
    }

    /// Guarda la contraseña de un host.
    pub fn guardar_contrasena(
        &self,
        alias: &str,
        usuario: &str,
        contrasena: &str,
    ) -> Resultado<()> {
        let atributos = Self::atributos(alias, usuario);
        let etiqueta = format!("Contraseña SSH de {usuario}@{alias}");

        self.ejecutor.block_on(async {
            let llavero = oo7::Keyring::new().await.map_err(traducir)?;
            llavero.unlock().await.map_err(traducir)?;
            llavero
                .create_item(&etiqueta, &&atributos, contrasena.as_bytes(), true)
                .await
                .map_err(traducir)
        })?;

        tracing::info!(
            alias = %alias,
            usuario = %usuario,
            "contraseña guardada en el llavero de la sesión"
        );
        Ok(())
    }

    /// Recupera la contraseña de un host, si la hay.
    pub fn leer_contrasena(&self, alias: &str, usuario: &str) -> Resultado<Option<String>> {
        let atributos = Self::atributos(alias, usuario);

        self.ejecutor.block_on(async {
            let llavero = oo7::Keyring::new().await.map_err(traducir)?;
            llavero.unlock().await.map_err(traducir)?;
            let elementos = llavero.search_items(&&atributos).await.map_err(traducir)?;
            match elementos.first() {
                Some(elemento) => {
                    let secreto = elemento.secret().await.map_err(traducir)?;
                    Ok(Some(String::from_utf8_lossy(&secreto).into_owned()))
                }
                None => Ok(None),
            }
        })
    }

    /// Borra la contraseña de un host.
    ///
    /// Es lo que se ofrece tras un `ssh-copy-id` correcto: si ya se entra con
    /// clave pública, la contraseña guardada solo es superficie de ataque.
    pub fn borrar_contrasena(&self, alias: &str, usuario: &str) -> Resultado<()> {
        let atributos = Self::atributos(alias, usuario);

        self.ejecutor.block_on(async {
            let llavero = oo7::Keyring::new().await.map_err(traducir)?;
            llavero.unlock().await.map_err(traducir)?;
            llavero.delete(&&atributos).await.map_err(traducir)
        })?;

        tracing::info!(alias = %alias, usuario = %usuario, "contraseña borrada del llavero");
        Ok(())
    }

    /// Alias que tienen alguna contraseña guardada.
    ///
    /// La interfaz los marca como «caso a extinguir»: son los hosts que todavía
    /// no usan clave pública.
    pub fn alias_con_contrasena(&self) -> Resultado<Vec<String>> {
        let mut filtro = HashMap::new();
        filtro.insert("aplicacion", APLICACION.to_string());

        self.ejecutor.block_on(async {
            let llavero = oo7::Keyring::new().await.map_err(traducir)?;
            llavero.unlock().await.map_err(traducir)?;
            let elementos = llavero.search_items(&&filtro).await.map_err(traducir)?;
            let mut alias = Vec::new();
            for elemento in elementos {
                if let Ok(atributos) = elemento.attributes().await {
                    if let Some(valor) = atributos.get("alias") {
                        alias.push(valor.clone());
                    }
                }
            }
            alias.sort();
            alias.dedup();
            Ok(alias)
        })
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn los_atributos_identifican_la_aplicacion_y_el_host() {
        let atributos = Llavero::atributos("preprod", "usuario");
        assert_eq!(
            atributos.get("aplicacion").map(String::as_str),
            Some(APLICACION)
        );
        assert_eq!(atributos.get("alias").map(String::as_str), Some("preprod"));
        assert_eq!(atributos.get("usuario").map(String::as_str), Some("usuario"));
    }

    #[test]
    fn el_ejecutor_se_construye_sin_llavero_delante() {
        // Abrir el llavero no debe requerir que haya un Secret Service: la
        // aplicación tiene que arrancar igual en una sesión sin llavero.
        assert!(Llavero::abrir().is_ok());
    }
}
