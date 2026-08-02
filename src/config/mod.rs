//! Fuente de verdad de la aplicación: `~/.ssh/config.d/yonder.conf` (§3.1).
//!
//! El catálogo reúne los hosts del fichero propio (editables) y los que ya
//! tenía el usuario en `~/.ssh/config` o en sus `Include` (solo lectura). Nunca
//! se mueven hosts de un fichero a otro: importar significa mostrarlos y poder
//! gestionarlos, no adoptarlos.

pub mod parser;
pub mod writer;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::{Error, Resultado};
use crate::modelo::{Host, Origen, Tunel};
use crate::rutas;

pub use parser::{Bloque, Documento, Encabezado, Linea};
pub use writer::{asegurar_include, estado_include, EstadoInclude};

/// Profundidad máxima al seguir directivas `Include`. OpenSSH permite 16.
const PROFUNDIDAD_MAXIMA: usize = 16;

/// Vista completa de la configuración SSH relevante para la aplicación.
#[derive(Debug, Clone)]
pub struct Catalogo {
    /// Fichero propio, con su texto original para poder reescribirlo sin pérdidas.
    pub propio: Documento,
    /// Hosts definidos en el fichero propio. Editables.
    pub propios: Vec<Host>,
    /// Hosts con reenvíos definidos fuera del fichero propio. Solo lectura.
    pub externos: Vec<Host>,
}

impl Catalogo {
    /// Carga el catálogo completo, creando el fichero propio si no existe.
    pub fn cargar() -> Resultado<Catalogo> {
        let propio = writer::asegurar_fichero_propio()?;
        let propios = propio.hosts(Origen::Propio);

        let conocidos: HashSet<String> = propios.iter().map(|h| h.alias.clone()).collect();
        let ruta_propia = rutas::config_tuneles()?;
        let mut externos = Vec::new();
        let mut visitados = HashSet::new();
        visitados.insert(ruta_propia.clone());

        let raiz = rutas::config_ssh_usuario()?;
        recolectar(&raiz, &mut visitados, &mut externos, 0);

        // Solo interesan los hosts que ya tengan algún reenvío: el resto son
        // entradas de conexión normales que no son túneles (§3.1).
        externos.retain(|h| !h.reenvios.is_empty() && !conocidos.contains(&h.alias));

        Ok(Catalogo {
            propio,
            propios,
            externos,
        })
    }

    /// Todos los hosts, propios primero.
    pub fn todos(&self) -> Vec<&Host> {
        self.propios.iter().chain(self.externos.iter()).collect()
    }

    pub fn host(&self, alias: &str) -> Option<&Host> {
        self.todos().into_iter().find(|h| h.alias == alias)
    }

    /// Todos los túneles definidos, en orden de fichero.
    pub fn tuneles(&self) -> Vec<Tunel> {
        self.todos().iter().flat_map(|h| h.tuneles()).collect()
    }

    pub fn tunel(&self, id: &str) -> Option<Tunel> {
        self.tuneles().into_iter().find(|t| t.id() == id)
    }

    /// Vuelca los cambios del documento propio al disco y recarga.
    pub fn guardar(&mut self) -> Resultado<()> {
        writer::guardar(&self.propio)?;
        self.propios = self.propio.hosts(Origen::Propio);
        Ok(())
    }

    /// Crea o actualiza un host en el fichero propio.
    pub fn guardar_host(&mut self, host: &Host) -> Resultado<()> {
        if let Some(existente) = self.externos.iter().find(|h| h.alias == host.alias) {
            return Err(Error::HostAjeno(existente.alias.clone()));
        }
        writer::sincronizar_host(&mut self.propio, host)?;
        self.guardar()
    }

    pub fn renombrar_host(&mut self, antiguo: &str, nuevo: &str) -> Resultado<()> {
        if self.externos.iter().any(|h| h.alias == nuevo) {
            return Err(Error::HostDuplicado(nuevo.to_string()));
        }
        writer::renombrar_host(&mut self.propio, antiguo, nuevo)?;
        self.guardar()
    }

    pub fn eliminar_host(&mut self, alias: &str) -> Resultado<()> {
        writer::eliminar_host(&mut self.propio, alias)?;
        self.guardar()
    }
}

/// Lee un fichero y sigue sus `Include` en profundidad.
///
/// Los errores se registran pero no abortan: un `Include` roto en un fichero
/// del usuario no debe impedir que la aplicación arranque.
fn recolectar(
    ruta: &Path,
    visitados: &mut HashSet<PathBuf>,
    salida: &mut Vec<Host>,
    profundidad: usize,
) {
    if profundidad >= PROFUNDIDAD_MAXIMA {
        tracing::warn!(ruta = %ruta.display(), "se alcanzó la profundidad máxima de Include");
        return;
    }
    let ruta = ruta.canonicalize().unwrap_or_else(|_| ruta.to_path_buf());
    if !visitados.insert(ruta.clone()) {
        return;
    }

    let documento = match Documento::leer(&ruta) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(ruta = %ruta.display(), "no se pudo leer: {e}");
            return;
        }
    };

    salida.extend(documento.hosts(Origen::Ajeno(ruta.clone())));

    for patron in documento.includes() {
        for incluido in expandir_patron(&patron) {
            recolectar(&incluido, visitados, salida, profundidad + 1);
        }
    }
}

/// Expande un `Include` con comodines en el último componente.
///
/// Cubre el caso real (`Include ~/.ssh/config.d/*.conf`) sin arrastrar una
/// dependencia de globbing por un patrón de una sola línea.
fn expandir_patron(patron: &Path) -> Vec<PathBuf> {
    let nombre = match patron.file_name().and_then(|n| n.to_str()) {
        Some(n) if n.contains(['*', '?']) => n,
        _ => return vec![patron.to_path_buf()],
    };
    let directorio = match patron.parent() {
        Some(d) => d,
        None => return Vec::new(),
    };
    let entradas = match std::fs::read_dir(directorio) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut encontrados: Vec<PathBuf> = entradas
        .flatten()
        .filter(|e| e.path().is_file())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| coincide(nombre, n))
                .unwrap_or(false)
        })
        .map(|e| e.path())
        .collect();
    encontrados.sort();
    encontrados
}

/// Coincidencia de comodines al estilo de la consola: `*` y `?`.
fn coincide(patron: &str, texto: &str) -> bool {
    let patron: Vec<char> = patron.chars().collect();
    let texto: Vec<char> = texto.chars().collect();
    // Programación dinámica clásica; los patrones aquí son de una línea.
    let mut posible = vec![vec![false; texto.len() + 1]; patron.len() + 1];
    posible[0][0] = true;
    for p in 1..=patron.len() {
        if patron[p - 1] == '*' {
            posible[p][0] = posible[p - 1][0];
        }
    }
    for p in 1..=patron.len() {
        for t in 1..=texto.len() {
            posible[p][t] = match patron[p - 1] {
                '*' => posible[p - 1][t] || posible[p][t - 1],
                '?' => posible[p - 1][t - 1],
                c => posible[p - 1][t - 1] && c == texto[t - 1],
            };
        }
    }
    posible[patron.len()][texto.len()]
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn el_comodin_se_comporta_como_en_la_consola() {
        assert!(coincide("*.conf", "yonder.conf"));
        assert!(coincide("*", "cualquiera"));
        assert!(coincide("config?", "config1"));
        assert!(!coincide("*.conf", "yonder.txt"));
        assert!(!coincide("config?", "config"));
    }
}
