//! Preferencias de la aplicación: `$XDG_CONFIG_HOME/yonder/config.toml` (§6).
//!
//! Aquí no hay definiciones de túneles: esas viven en `ssh_config` y solo ahí
//! (§3.1). Esto es cómo se ve y cómo se comporta la ventana.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Resultado};
use crate::rutas;

/// Preferencia de tema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Tema {
    /// Sigue lo que diga el escritorio.
    #[default]
    Auto,
    Claro,
    Oscuro,
}

impl Tema {
    pub fn etiqueta(&self) -> &'static str {
        match self {
            Tema::Auto => "Automático",
            Tema::Claro => "Claro",
            Tema::Oscuro => "Oscuro",
        }
    }

    pub fn siguiente(&self) -> Tema {
        match self {
            Tema::Auto => Tema::Claro,
            Tema::Claro => Tema::Oscuro,
            Tema::Oscuro => Tema::Auto,
        }
    }
}

/// Densidad de la interfaz.
///
/// Modula la escala de espaciado, no rehace los componentes: es un multiplicador
/// sobre la rejilla de 4·n.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Densidad {
    Compacta,
    #[default]
    Comoda,
}

impl Densidad {
    pub fn factor(&self) -> f32 {
        match self {
            Densidad::Compacta => 0.6,
            Densidad::Comoda => 1.0,
        }
    }

    pub fn etiqueta(&self) -> &'static str {
        match self {
            Densidad::Compacta => "Compacta",
            Densidad::Comoda => "Cómoda",
        }
    }

    pub fn siguiente(&self) -> Densidad {
        match self {
            Densidad::Compacta => Densidad::Comoda,
            Densidad::Comoda => Densidad::Compacta,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferencias {
    pub tema: Tema,
    pub densidad: Densidad,
    /// Levantar los túneles marcados al abrir la ventana.
    pub autoarranque_al_abrir: bool,
    /// Segundos de `ServerAliveInterval` (§3.3).
    pub intervalo_latido: u32,
    /// `ServerAliveCountMax` (§3.3).
    pub latidos_perdidos: u32,
    /// Segundos de `ConnectTimeout`.
    pub espera_conexion: u32,
    /// Reintentos antes de dar un túnel por fallido. 0 = sin límite.
    pub maximo_reintentos: u32,
    /// Segundos entre sondas de salud profundas.
    ///
    /// Las comprobaciones `banner` y `http` abren conexiones reales a través
    /// del túnel: tienen efecto en el servicio del otro lado. Subir este valor
    /// molesta menos; bajarlo detecta antes el túnel zombi.
    pub intervalo_salud: u32,
    /// Confirmar antes de bajar un túnel activo.
    pub confirmar_al_bajar: bool,
    /// Mostrar la columna de estadísticas en la lista.
    pub mostrar_estadisticas: bool,
    /// Escala de la interfaz. 1.0 es el tamaño base.
    ///
    /// Se aplica con el zoom de egui, que escala la letra **y** el espaciado a
    /// la vez. Subir solo el tamaño de letra dejaría los textos grandes dentro
    /// de cajas pensadas para los pequeños, y lo que se gana en legibilidad se
    /// pierde en recortes.
    pub escala_interfaz: f32,
}

impl Default for Preferencias {
    fn default() -> Self {
        Preferencias {
            tema: Tema::default(),
            densidad: Densidad::default(),
            autoarranque_al_abrir: true,
            intervalo_latido: 15,
            latidos_perdidos: 3,
            espera_conexion: 15,
            maximo_reintentos: 10,
            intervalo_salud: 30,
            confirmar_al_bajar: false,
            mostrar_estadisticas: true,
            escala_interfaz: 1.0,
        }
    }
}

impl Preferencias {
    /// Carga las preferencias. Si el fichero no existe o está roto, se usan los
    /// valores por defecto y se avisa: una preferencia mal escrita no debe
    /// impedir que la aplicación abra.
    pub fn cargar() -> Preferencias {
        let ruta = match rutas::preferencias() {
            Ok(ruta) => ruta,
            Err(e) => {
                tracing::warn!("no se pudo determinar la ruta de preferencias: {e}");
                return Preferencias::default();
            }
        };
        match std::fs::read_to_string(&ruta) {
            Ok(texto) => match toml::from_str(&texto) {
                Ok(preferencias) => preferencias,
                Err(e) => {
                    tracing::warn!(
                        ruta = %rutas::abreviar(&ruta),
                        "preferencias ilegibles, se usan las de por defecto: {e}"
                    );
                    Preferencias::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Preferencias::default(),
            Err(e) => {
                tracing::warn!(ruta = %rutas::abreviar(&ruta), "no se pudo leer: {e}");
                Preferencias::default()
            }
        }
    }

    pub fn guardar(&self) -> Resultado<()> {
        let ruta = rutas::preferencias()?;
        if let Some(directorio) = ruta.parent() {
            rutas::asegurar_directorio_privado(directorio)?;
        }
        let texto = toml::to_string_pretty(self)
            .map_err(|e| Error::DefinicionInvalida(format!("no se pudo serializar: {e}")))?;
        let cabecera = "# Preferencias de «yonder».\n\
                        # Las definiciones de túneles NO están aquí: viven en\n\
                        # ~/.ssh/config.d/yonder.conf.\n\n";
        std::fs::write(&ruta, format!("{cabecera}{texto}"))
            .map_err(|e| Error::escribiendo(&ruta, e))?;
        Ok(())
    }

    /// Opciones del maestro derivadas de las preferencias.
    pub fn opciones_maestro(&self) -> crate::ssh::OpcionesMaestro {
        crate::ssh::OpcionesMaestro {
            espera_conexion: self.espera_conexion.clamp(3, 300),
            intervalo_latido: self.intervalo_latido.clamp(5, 300),
            latidos_perdidos: self.latidos_perdidos.clamp(1, 20),
            sin_interaccion: false,
        }
    }

    /// Cadencia de la sonda profunda de salud.
    pub fn intervalo_salud(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.intervalo_salud.clamp(5, 3600) as u64)
    }

    /// Política de reintento derivada de las preferencias (§3.3).
    pub fn politica_reintento(&self) -> crate::state::PoliticaReintento {
        crate::state::PoliticaReintento {
            maximo_intentos: self.maximo_reintentos,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn los_valores_por_defecto_son_los_de_la_documentacion() {
        let preferencias = Preferencias::default();
        assert_eq!(preferencias.intervalo_latido, 15);
        assert_eq!(preferencias.latidos_perdidos, 3);
    }

    #[test]
    fn la_ida_y_vuelta_por_toml_es_estable() {
        let original = Preferencias {
            tema: Tema::Oscuro,
            densidad: Densidad::Compacta,
            maximo_reintentos: 0,
            ..Default::default()
        };
        let texto = toml::to_string_pretty(&original).unwrap();
        let vuelta: Preferencias = toml::from_str(&texto).unwrap();
        assert_eq!(original, vuelta);
    }

    #[test]
    fn un_toml_incompleto_rellena_con_los_valores_por_defecto() {
        let vuelta: Preferencias = toml::from_str("tema = \"oscuro\"").unwrap();
        assert_eq!(vuelta.tema, Tema::Oscuro);
        assert_eq!(vuelta.intervalo_latido, 15);
    }

    #[test]
    fn los_valores_absurdos_se_acotan_al_construir_las_opciones() {
        let preferencias = Preferencias {
            espera_conexion: 0,
            intervalo_latido: 100_000,
            latidos_perdidos: 0,
            ..Default::default()
        };
        let opciones = preferencias.opciones_maestro();
        assert_eq!(opciones.espera_conexion, 3);
        assert_eq!(opciones.intervalo_latido, 300);
        assert_eq!(opciones.latidos_perdidos, 1);
    }

    #[test]
    fn la_cadencia_de_salud_se_acota() {
        let bajo = Preferencias {
            intervalo_salud: 0,
            ..Default::default()
        };
        // Sondear cada cero segundos sería martillear el servicio remoto.
        assert_eq!(bajo.intervalo_salud().as_secs(), 5);

        let alto = Preferencias {
            intervalo_salud: 999_999,
            ..Default::default()
        };
        assert_eq!(alto.intervalo_salud().as_secs(), 3600);
    }

    #[test]
    fn el_tema_rota_y_vuelve_al_principio() {
        assert_eq!(Tema::Auto.siguiente().siguiente().siguiente(), Tema::Auto);
    }
}
