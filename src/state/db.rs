//! Estado de ejecución y preferencias en SQLite (§3.4).
//!
//! **Aquí no hay credenciales.** Ni contraseñas, ni claves, ni passphrases.
//! Solo orden en la lista, marca de autoarranque, última conexión correcta,
//! contador de fallos y estadísticas.
//!
//! Un apunte sobre §2.4 («sin estado propio duplicado»): la tabla guarda una
//! columna `deseado`, que **no** duplica nada. El socket de control sabe si el
//! maestro está vivo, pero no sabe si un reenvío concreto está bajado porque se
//! cayó o porque el usuario nunca lo levantó. Esa intención solo la conoce la
//! aplicación, y es lo que permite distinguir `Definido` de `Degradado` tras
//! reabrir la ventana.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{Error, Resultado};
use crate::rutas;

/// Versión del esquema. Se sube cuando hay que migrar.
const VERSION_ESQUEMA: i64 = 1;

fn traducir(e: rusqlite::Error) -> Error {
    Error::BaseDeDatos(e.to_string())
}

fn ahora() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Fila de estado de un túnel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistroTunel {
    pub id: String,
    /// Posición en la lista de la interfaz.
    pub orden: i64,
    /// El usuario quiere este túnel arriba.
    pub deseado: bool,
    /// Levantar al arrancar la aplicación.
    pub autoarranque: bool,
    /// Marca de tiempo Unix de la última conexión correcta.
    pub ultima_conexion: Option<i64>,
    pub fallos: i64,
    pub activaciones: i64,
    pub segundos_activo: i64,
}

impl RegistroTunel {
    pub fn nuevo(id: impl Into<String>) -> RegistroTunel {
        RegistroTunel {
            id: id.into(),
            orden: 0,
            deseado: false,
            autoarranque: false,
            ultima_conexion: None,
            fallos: 0,
            activaciones: 0,
            segundos_activo: 0,
        }
    }
}

/// Base de datos de estado.
pub struct BaseDeDatos {
    conexion: Connection,
}

impl std::fmt::Debug for BaseDeDatos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BaseDeDatos").finish_non_exhaustive()
    }
}

impl BaseDeDatos {
    /// Abre (creando si hace falta) la base en `$XDG_DATA_HOME/yonder/estado.db`.
    pub fn abrir() -> Resultado<BaseDeDatos> {
        let ruta = rutas::base_de_datos()?;
        if let Some(directorio) = ruta.parent() {
            rutas::asegurar_directorio_privado(directorio)?;
        }
        BaseDeDatos::abrir_en(&ruta)
    }

    pub fn abrir_en(ruta: &Path) -> Resultado<BaseDeDatos> {
        let conexion = Connection::open(ruta).map_err(traducir)?;
        let base = BaseDeDatos { conexion };
        base.migrar()?;
        Ok(base)
    }

    /// Base en memoria, para pruebas.
    pub fn en_memoria() -> Resultado<BaseDeDatos> {
        let conexion = Connection::open_in_memory().map_err(traducir)?;
        let base = BaseDeDatos { conexion };
        base.migrar()?;
        Ok(base)
    }

    fn migrar(&self) -> Resultado<()> {
        self.conexion
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;

                 CREATE TABLE IF NOT EXISTS meta (
                     clave TEXT PRIMARY KEY,
                     valor TEXT NOT NULL
                 );

                 CREATE TABLE IF NOT EXISTS tuneles (
                     id              TEXT PRIMARY KEY,
                     orden           INTEGER NOT NULL DEFAULT 0,
                     deseado         INTEGER NOT NULL DEFAULT 0,
                     autoarranque    INTEGER NOT NULL DEFAULT 0,
                     ultima_conexion INTEGER,
                     fallos          INTEGER NOT NULL DEFAULT 0,
                     activaciones    INTEGER NOT NULL DEFAULT 0,
                     segundos_activo INTEGER NOT NULL DEFAULT 0
                 );

                 CREATE TABLE IF NOT EXISTS hosts_externos (
                     alias        TEXT PRIMARY KEY,
                     visible      INTEGER NOT NULL DEFAULT 0,
                     importado_en INTEGER
                 );",
            )
            .map_err(traducir)?;

        let version: Option<i64> = self
            .conexion
            .query_row(
                "SELECT CAST(valor AS INTEGER) FROM meta WHERE clave = 'version_esquema'",
                [],
                |fila| fila.get(0),
            )
            .optional()
            .map_err(traducir)?;

        if version.is_none() {
            self.conexion
                .execute(
                    "INSERT INTO meta (clave, valor) VALUES ('version_esquema', ?1)",
                    params![VERSION_ESQUEMA.to_string()],
                )
                .map_err(traducir)?;
        }
        Ok(())
    }

    /// Registro de un túnel; uno nuevo con valores por defecto si no existía.
    pub fn tunel(&self, id: &str) -> Resultado<RegistroTunel> {
        self.conexion
            .query_row(
                "SELECT id, orden, deseado, autoarranque, ultima_conexion,
                        fallos, activaciones, segundos_activo
                 FROM tuneles WHERE id = ?1",
                params![id],
                |fila| {
                    Ok(RegistroTunel {
                        id: fila.get(0)?,
                        orden: fila.get(1)?,
                        deseado: fila.get::<_, i64>(2)? != 0,
                        autoarranque: fila.get::<_, i64>(3)? != 0,
                        ultima_conexion: fila.get(4)?,
                        fallos: fila.get(5)?,
                        activaciones: fila.get(6)?,
                        segundos_activo: fila.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(traducir)
            .map(|fila| fila.unwrap_or_else(|| RegistroTunel::nuevo(id)))
    }

    pub fn todos(&self) -> Resultado<Vec<RegistroTunel>> {
        let mut consulta = self
            .conexion
            .prepare(
                "SELECT id, orden, deseado, autoarranque, ultima_conexion,
                        fallos, activaciones, segundos_activo
                 FROM tuneles ORDER BY orden, id",
            )
            .map_err(traducir)?;
        let filas = consulta
            .query_map([], |fila| {
                Ok(RegistroTunel {
                    id: fila.get(0)?,
                    orden: fila.get(1)?,
                    deseado: fila.get::<_, i64>(2)? != 0,
                    autoarranque: fila.get::<_, i64>(3)? != 0,
                    ultima_conexion: fila.get(4)?,
                    fallos: fila.get(5)?,
                    activaciones: fila.get(6)?,
                    segundos_activo: fila.get(7)?,
                })
            })
            .map_err(traducir)?;
        filas.collect::<Result<Vec<_>, _>>().map_err(traducir)
    }

    pub fn guardar(&self, registro: &RegistroTunel) -> Resultado<()> {
        self.conexion
            .execute(
                "INSERT INTO tuneles
                    (id, orden, deseado, autoarranque, ultima_conexion,
                     fallos, activaciones, segundos_activo)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                    orden = excluded.orden,
                    deseado = excluded.deseado,
                    autoarranque = excluded.autoarranque,
                    ultima_conexion = excluded.ultima_conexion,
                    fallos = excluded.fallos,
                    activaciones = excluded.activaciones,
                    segundos_activo = excluded.segundos_activo",
                params![
                    registro.id,
                    registro.orden,
                    i64::from(registro.deseado),
                    i64::from(registro.autoarranque),
                    registro.ultima_conexion,
                    registro.fallos,
                    registro.activaciones,
                    registro.segundos_activo,
                ],
            )
            .map_err(traducir)?;
        Ok(())
    }

    /// Marca la intención del usuario sobre un túnel.
    pub fn fijar_deseado(&self, id: &str, deseado: bool) -> Resultado<()> {
        let mut registro = self.tunel(id)?;
        registro.deseado = deseado;
        self.guardar(&registro)
    }

    pub fn fijar_autoarranque(&self, id: &str, autoarranque: bool) -> Resultado<()> {
        let mut registro = self.tunel(id)?;
        registro.autoarranque = autoarranque;
        self.guardar(&registro)
    }

    /// Apunta una conexión correcta.
    pub fn apuntar_exito(&self, id: &str) -> Resultado<()> {
        let mut registro = self.tunel(id)?;
        registro.ultima_conexion = Some(ahora());
        registro.activaciones += 1;
        registro.fallos = 0;
        self.guardar(&registro)
    }

    /// Apunta un fallo.
    pub fn apuntar_fallo(&self, id: &str) -> Resultado<()> {
        let mut registro = self.tunel(id)?;
        registro.fallos += 1;
        self.guardar(&registro)
    }

    /// Suma tiempo activo a las estadísticas.
    pub fn sumar_tiempo_activo(&self, id: &str, segundos: i64) -> Resultado<()> {
        if segundos <= 0 {
            return Ok(());
        }
        let mut registro = self.tunel(id)?;
        registro.segundos_activo += segundos;
        self.guardar(&registro)
    }

    /// Fija el orden de la lista de una vez.
    pub fn fijar_orden(&self, ids: &[String]) -> Resultado<()> {
        for (posicion, id) in ids.iter().enumerate() {
            let mut registro = self.tunel(id)?;
            registro.orden = posicion as i64;
            self.guardar(&registro)?;
        }
        Ok(())
    }

    /// Borra registros de túneles que ya no existen en la configuración.
    pub fn purgar(&self, ids_vigentes: &[String]) -> Resultado<usize> {
        let existentes = self.todos()?;
        let mut borrados = 0;
        for registro in existentes {
            if !ids_vigentes.contains(&registro.id) {
                self.conexion
                    .execute("DELETE FROM tuneles WHERE id = ?1", params![registro.id])
                    .map_err(traducir)?;
                borrados += 1;
            }
        }
        Ok(borrados)
    }

    // --- Hosts externos (§3.1: importar es mostrar, no mover de fichero) ---

    /// `true` si el usuario ya decidió mostrar este host externo.
    pub fn externo_visible(&self, alias: &str) -> Resultado<bool> {
        self.conexion
            .query_row(
                "SELECT visible FROM hosts_externos WHERE alias = ?1",
                params![alias],
                |fila| fila.get::<_, i64>(0),
            )
            .optional()
            .map_err(traducir)
            .map(|v| v.unwrap_or(0) != 0)
    }

    pub fn fijar_externo_visible(&self, alias: &str, visible: bool) -> Resultado<()> {
        self.conexion
            .execute(
                "INSERT INTO hosts_externos (alias, visible, importado_en)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(alias) DO UPDATE SET
                    visible = excluded.visible,
                    importado_en = excluded.importado_en",
                params![alias, i64::from(visible), ahora()],
            )
            .map_err(traducir)?;
        Ok(())
    }

    /// `true` si el usuario ya ha pasado por la importación inicial.
    pub fn importacion_hecha(&self) -> Resultado<bool> {
        self.marca("importacion_hecha")
    }

    pub fn marcar_importacion_hecha(&self) -> Resultado<()> {
        self.fijar_marca("importacion_hecha", true)
    }

    /// `true` si ya se comprobó la línea `Include` de `~/.ssh/config`.
    pub fn include_comprobado(&self) -> Resultado<bool> {
        self.marca("include_comprobado")
    }

    pub fn marcar_include_comprobado(&self) -> Resultado<()> {
        self.fijar_marca("include_comprobado", true)
    }

    fn marca(&self, clave: &str) -> Resultado<bool> {
        self.conexion
            .query_row(
                "SELECT valor FROM meta WHERE clave = ?1",
                params![clave],
                |fila| fila.get::<_, String>(0),
            )
            .optional()
            .map_err(traducir)
            .map(|v| v.as_deref() == Some("1"))
    }

    fn fijar_marca(&self, clave: &str, valor: bool) -> Resultado<()> {
        self.conexion
            .execute(
                "INSERT INTO meta (clave, valor) VALUES (?1, ?2)
                 ON CONFLICT(clave) DO UPDATE SET valor = excluded.valor",
                params![clave, if valor { "1" } else { "0" }],
            )
            .map_err(traducir)?;
        Ok(())
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn un_tunel_desconocido_devuelve_valores_por_defecto() {
        let base = BaseDeDatos::en_memoria().unwrap();
        let registro = base.tunel("no-existe").unwrap();
        assert_eq!(registro.id, "no-existe");
        assert!(!registro.deseado);
        assert_eq!(registro.fallos, 0);
    }

    #[test]
    fn la_intencion_del_usuario_sobrevive() {
        let base = BaseDeDatos::en_memoria().unwrap();
        base.fijar_deseado("preprod|-L|3000", true).unwrap();
        assert!(base.tunel("preprod|-L|3000").unwrap().deseado);
        base.fijar_deseado("preprod|-L|3000", false).unwrap();
        assert!(!base.tunel("preprod|-L|3000").unwrap().deseado);
    }

    #[test]
    fn el_exito_pone_a_cero_los_fallos() {
        let base = BaseDeDatos::en_memoria().unwrap();
        base.apuntar_fallo("t").unwrap();
        base.apuntar_fallo("t").unwrap();
        assert_eq!(base.tunel("t").unwrap().fallos, 2);
        base.apuntar_exito("t").unwrap();
        let registro = base.tunel("t").unwrap();
        assert_eq!(registro.fallos, 0);
        assert_eq!(registro.activaciones, 1);
        assert!(registro.ultima_conexion.is_some());
    }

    #[test]
    fn el_orden_se_guarda_y_se_lee() {
        let base = BaseDeDatos::en_memoria().unwrap();
        base.fijar_orden(&["c".into(), "a".into(), "b".into()])
            .unwrap();
        let ids: Vec<String> = base.todos().unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["c", "a", "b"]);
    }

    #[test]
    fn purgar_borra_lo_que_ya_no_existe() {
        let base = BaseDeDatos::en_memoria().unwrap();
        base.fijar_deseado("vivo", true).unwrap();
        base.fijar_deseado("muerto", true).unwrap();
        let borrados = base.purgar(&["vivo".to_string()]).unwrap();
        assert_eq!(borrados, 1);
        assert_eq!(base.todos().unwrap().len(), 1);
    }

    #[test]
    fn las_marcas_de_primera_ejecucion_persisten() {
        let base = BaseDeDatos::en_memoria().unwrap();
        assert!(!base.importacion_hecha().unwrap());
        base.marcar_importacion_hecha().unwrap();
        assert!(base.importacion_hecha().unwrap());
    }

    #[test]
    fn los_hosts_externos_empiezan_ocultos() {
        let base = BaseDeDatos::en_memoria().unwrap();
        assert!(!base.externo_visible("viejo").unwrap());
        base.fijar_externo_visible("viejo", true).unwrap();
        assert!(base.externo_visible("viejo").unwrap());
    }

    #[test]
    fn el_fichero_se_crea_y_se_reabre() {
        let directorio = tempfile::tempdir().unwrap();
        let ruta = directorio.path().join("estado.db");
        {
            let base = BaseDeDatos::abrir_en(&ruta).unwrap();
            base.fijar_autoarranque("t", true).unwrap();
        }
        let base = BaseDeDatos::abrir_en(&ruta).unwrap();
        assert!(base.tunel("t").unwrap().autoarranque);
    }
}
