//! Escritura de `~/.ssh/config.d/yonder.conf` y gestión del `Include`.
//!
//! Reglas de §3.1 que este módulo hace cumplir:
//!
//! - La aplicación es dueña **exclusiva** del fichero propio.
//! - `~/.ssh/config` nunca se reescribe, salvo para añadir la línea `Include`
//!   la primera vez, con copia de seguridad previa y comprobando que no exista ya.
//! - Al reescribir el fichero propio se preservan comentarios y orden.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use crate::error::{Error, Resultado};
use crate::modelo::{Host, Reenvio, Salud, TipoReenvio};
use crate::rutas;

use super::parser::{Bloque, Clase, Documento, Encabezado, Linea};

/// Cabecera que la aplicación escribe al crear su fichero.
const CABECERA: &[&str] = &[
    "# Fichero gestionado por «yonder».",
    "#",
    "# Se puede editar a mano: la aplicación conserva los comentarios, el orden",
    "# y cualquier directiva que no entienda. Lo que no se debe hacer es mover",
    "# aquí hosts de otros ficheros esperando que sigan estando allí.",
    "#",
    "# Cada Host definido aquí funciona también con ssh, scp, rsync y VS Code",
    "# Remote sin que la aplicación esté corriendo. Ese es justamente el punto.",
    "#",
    "# Ejemplos comentados, del túnel simple al Oracle a dos saltos:",
    "#     /usr/share/doc/yonder/ejemplo.conf   (example.conf, in English)",
];

/// Directivas de robustez que la aplicación mantiene en cada host (§3.3).
const ROBUSTEZ: &[(&str, &str)] = &[
    ("ExitOnForwardFailure", "yes"),
    ("ServerAliveInterval", "15"),
    ("ServerAliveCountMax", "3"),
    // Detecta la caída a nivel de TCP además de a nivel de SSH. Los
    // ServerAlive* viajan cifrados y cubren el caso normal; TCPKeepAlive cubre
    // el del cortafuegos que descarta la conexión sin avisar a nadie.
    ("TCPKeepAlive", "yes"),
];

/// Directivas cuyo contenido gestiona la aplicación. El resto no se toca.
fn es_gestionada(clave: &str) -> bool {
    matches!(
        clave,
        "hostname"
            | "user"
            | "port"
            | "proxyjump"
            | "identityfile"
            | "localforward"
            | "remoteforward"
            | "dynamicforward"
            | "exitonforwardfailure"
            | "serveraliveinterval"
            | "serveralivecountmax"
            | "tcpkeepalive"
    )
}

/// Escribe el documento de forma atómica y con permisos 0600.
///
/// Se escribe a un temporal en el mismo directorio y se renombra: si algo falla
/// a mitad, el fichero anterior sigue intacto. Un `ssh_config` a medio escribir
/// deja al usuario sin poder conectarse a nada.
pub fn guardar(documento: &Documento) -> Resultado<()> {
    let ruta = &documento.ruta;
    let directorio = ruta.parent().ok_or_else(|| {
        Error::DefinicionInvalida(format!("«{}» no tiene directorio", ruta.display()))
    })?;

    if !directorio.exists() {
        std::fs::create_dir_all(directorio).map_err(|e| Error::escribiendo(directorio, e))?;
        let permisos = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(directorio, permisos)
            .map_err(|e| Error::escribiendo(directorio, e))?;
    }

    let temporal = directorio.join(format!(
        ".{}.tmp",
        ruta.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("yonder.conf")
    ));

    {
        let mut fichero = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporal)
            .map_err(|e| Error::escribiendo(&temporal, e))?;
        fichero
            .write_all(documento.a_texto().as_bytes())
            .map_err(|e| Error::escribiendo(&temporal, e))?;
        fichero
            .sync_all()
            .map_err(|e| Error::escribiendo(&temporal, e))?;
    }

    std::fs::rename(&temporal, ruta).map_err(|e| Error::escribiendo(ruta, e))?;
    tracing::debug!(ruta = %ruta.display(), "fichero de túneles guardado");
    Ok(())
}

/// Crea el fichero propio con su cabecera si todavía no existe.
pub fn asegurar_fichero_propio() -> Resultado<Documento> {
    let ruta = rutas::config_tuneles()?;
    let mut documento = Documento::leer(&ruta)?;
    if documento.preambulo.is_empty() && documento.bloques.is_empty() {
        for linea in CABECERA {
            documento.preambulo.push(Linea::analizar(linea));
        }
        documento.preambulo.push(Linea::vacia());
        guardar(&documento)?;
        tracing::info!(ruta = %rutas::abreviar(&ruta), "creado el fichero de túneles");
    }
    Ok(documento)
}

/// Resultado de comprobar el `Include` en `~/.ssh/config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EstadoInclude {
    /// La línea ya está y apunta a nuestro fichero.
    Presente,
    /// Falta y hay que añadirla.
    Ausente,
    /// No existe `~/.ssh/config`; se creará entero.
    SinFichero,
}

/// Comprueba si `~/.ssh/config` ya incluye el fichero propio.
pub fn estado_include() -> Resultado<EstadoInclude> {
    let ruta = rutas::config_ssh_usuario()?;
    if !ruta.exists() {
        return Ok(EstadoInclude::SinFichero);
    }
    let documento = Documento::leer(&ruta)?;
    let objetivo = rutas::config_tuneles()?;
    let ya_esta = documento
        .includes()
        .iter()
        .any(|incluido| incluido == &objetivo);
    Ok(if ya_esta {
        EstadoInclude::Presente
    } else {
        EstadoInclude::Ausente
    })
}

/// Añade `Include ~/.ssh/config.d/yonder.conf` como **primera línea** de
/// `~/.ssh/config`, con copia de seguridad previa.
///
/// Tiene que ir la primera: en `ssh_config` gana el primer valor que se
/// encuentra para cada directiva, así que un `Include` al final quedaría
/// eclipsado por cualquier `Host *` anterior.
///
/// Devuelve la ruta de la copia de seguridad, si se ha creado alguna.
pub fn asegurar_include() -> Resultado<Option<std::path::PathBuf>> {
    match estado_include()? {
        EstadoInclude::Presente => return Ok(None),
        EstadoInclude::SinFichero => {
            let ruta = rutas::config_ssh_usuario()?;
            let mut documento = Documento::vacio(&ruta);
            documento
                .preambulo
                .push(Linea::analizar(rutas::LINEA_INCLUDE));
            documento.preambulo.push(Linea::vacia());
            guardar(&documento)?;
            tracing::info!(ruta = %rutas::abreviar(&ruta), "creado ~/.ssh/config con el Include");
            return Ok(None);
        }
        EstadoInclude::Ausente => {}
    }

    let ruta = rutas::config_ssh_usuario()?;
    let respaldo = respaldar(&ruta)?;

    let mut documento = Documento::leer(&ruta)?;
    let mut nuevo_preambulo = vec![
        Linea::analizar(rutas::LINEA_INCLUDE),
        Linea::comentario("línea añadida por «yonder»; el resto del fichero no se ha tocado"),
        Linea::vacia(),
    ];
    nuevo_preambulo.append(&mut documento.preambulo);
    documento.preambulo = nuevo_preambulo;
    guardar(&documento)?;

    tracing::info!(
        ruta = %rutas::abreviar(&ruta),
        respaldo = %rutas::abreviar(&respaldo),
        "añadida la línea Include"
    );
    Ok(Some(respaldo))
}

/// Copia de seguridad con marca temporal junto al original.
///
/// Se fuerza el 0600 en lugar de heredar los permisos del original: si el
/// `~/.ssh/config` del usuario estaba mal protegido, la copia no va a repetir
/// el error. Un fichero de configuración SSH legible por otros filtra nombres
/// de host, usuarios y rutas de claves.
fn respaldar(ruta: &Path) -> Resultado<std::path::PathBuf> {
    let marca = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let destino = ruta.with_extension(format!("respaldo-{marca}"));
    std::fs::copy(ruta, &destino).map_err(|e| Error::escribiendo(&destino, e))?;
    std::fs::set_permissions(&destino, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| Error::escribiendo(&destino, e))?;
    Ok(destino)
}

/// Inserta o actualiza un host en el documento, preservando todo lo demás.
pub fn sincronizar_host(documento: &mut Documento, host: &Host) -> Resultado<()> {
    host.validar()?;
    match documento.posicion(&host.alias) {
        Some(indice) => sincronizar_bloque(&mut documento.bloques[indice], host),
        None => {
            if !documento.bloques.is_empty() || !documento.preambulo.is_empty() {
                asegurar_linea_en_blanco_final(documento);
            }
            documento.bloques.push(bloque_nuevo(host));
        }
    }
    Ok(())
}

/// Renombra un host conservando el resto de su bloque.
pub fn renombrar_host(documento: &mut Documento, antiguo: &str, nuevo: &str) -> Resultado<()> {
    if antiguo == nuevo {
        return Ok(());
    }
    if documento.bloque(nuevo).is_some() {
        return Err(Error::HostDuplicado(nuevo.to_string()));
    }
    let bloque = documento
        .bloque_mut(antiguo)
        .ok_or_else(|| Error::HostDesconocido(antiguo.to_string()))?;
    bloque.encabezado = Encabezado::Host {
        patrones: vec![nuevo.to_string()],
    };
    bloque.linea_encabezado = Linea::analizar(&format!("Host {nuevo}"));
    Ok(())
}

/// Elimina un host y los comentarios que le pertenecen.
pub fn eliminar_host(documento: &mut Documento, alias: &str) -> Resultado<()> {
    let indice = documento
        .posicion(alias)
        .ok_or_else(|| Error::HostDesconocido(alias.to_string()))?;
    documento.bloques.remove(indice);
    Ok(())
}

fn asegurar_linea_en_blanco_final(documento: &mut Documento) {
    let ultimas = match documento.bloques.last_mut() {
        Some(bloque) => &mut bloque.cuerpo,
        None => &mut documento.preambulo,
    };
    if ultimas.last().map(|l| l.clase.clone()) != Some(Clase::Vacia) {
        ultimas.push(Linea::vacia());
    }
}

fn bloque_nuevo(host: &Host) -> Bloque {
    let mut bloque = Bloque {
        comentarios: Vec::new(),
        encabezado: Encabezado::Host {
            patrones: vec![host.alias.clone()],
        },
        linea_encabezado: Linea::analizar(&format!("Host {}", host.alias)),
        cuerpo: Vec::new(),
    };
    sincronizar_bloque(&mut bloque, host);
    bloque
}

/// Actualiza las directivas gestionadas de un bloque dejando intacto el resto.
///
/// El algoritmo respeta la posición: si una directiva ya existía, el valor
/// nuevo se escribe en su sitio; si desaparece, se borran sus líneas; si es
/// nueva, se añade al final de la zona de directivas, antes de los comentarios
/// de cierre.
fn sincronizar_bloque(bloque: &mut Bloque, host: &Host) {
    // Nota del usuario: comentario `# nota: …` justo bajo el encabezado.
    bloque.cuerpo.retain(|l| {
        !(l.clase == Clase::Comentario
            && l.cruda
                .trim()
                .trim_start_matches('#')
                .trim()
                .starts_with("nota:"))
    });
    if let Some(nota) = host.nota.as_ref().filter(|n| !n.trim().is_empty()) {
        bloque
            .cuerpo
            .insert(0, Linea::analizar(&format!("    # nota: {}", nota.trim())));
    }

    fijar_unica(bloque, "HostName", host.hostname.as_deref());
    fijar_unica(bloque, "User", host.usuario.as_deref());
    fijar_unica(
        bloque,
        "Port",
        host.puerto.map(|p| p.to_string()).as_deref(),
    );
    fijar_unica(
        bloque,
        "ProxyJump",
        if host.saltos.is_empty() {
            None
        } else {
            Some(host.saltos.join(","))
        }
        .as_deref(),
    );

    fijar_multiple(bloque, "IdentityFile", &host.identidades);

    for (tipo, clave) in [
        (TipoReenvio::Local, "LocalForward"),
        (TipoReenvio::Remoto, "RemoteForward"),
        (TipoReenvio::Dinamico, "DynamicForward"),
    ] {
        let del_tipo: Vec<&Reenvio> = host.reenvios.iter().filter(|r| r.tipo == tipo).collect();
        fijar_reenvios(bloque, clave, &del_tipo);
    }

    for (clave, valor) in ROBUSTEZ {
        fijar_unica(bloque, clave, Some(valor));
    }
}

/// `true` si la línea es un marcador `# salud: …`.
fn es_marca_de_salud(linea: &Linea) -> bool {
    linea.marca_de_salud().is_some()
}

/// `true` si la línea es un marcador `# nombre: …`.
fn es_marca_de_nombre(linea: &Linea) -> bool {
    linea.marca_de_nombre().is_some()
}

/// Reescribe los reenvíos de un tipo junto con sus marcas de salud.
///
/// Va aparte de `fijar_multiple` porque un reenvío no es una línea sino,
/// potencialmente, dos: el marcador `# salud:` y la directiva. Tratarlas por
/// separado dejaría marcas huérfanas apuntando al reenvío equivocado, que es
/// peor que no tener marca: mentiría sobre qué se está comprobando.
fn fijar_reenvios(bloque: &mut Bloque, clave: &str, reenvios: &[&Reenvio]) {
    let normalizada = clave.to_ascii_lowercase();

    let posiciones: Vec<usize> = bloque
        .cuerpo
        .iter()
        .enumerate()
        .filter(|(_, l)| l.clave_normalizada().as_deref() == Some(normalizada.as_str()))
        .map(|(i, _)| i)
        .collect();

    let destino = posiciones
        .first()
        .copied()
        .unwrap_or_else(|| punto_de_insercion(bloque));

    // Se borra cada directiva y las marcas que la preceden, que pueden ser dos:
    // el nombre y la salud. Se retrocede mientras haya marcas para no dejar
    // huérfana la de más arriba.
    let mut a_borrar: Vec<usize> = Vec::new();
    for &posicion in &posiciones {
        a_borrar.push(posicion);
        let mut anterior = posicion;
        while anterior > 0
            && (es_marca_de_salud(&bloque.cuerpo[anterior - 1])
                || es_marca_de_nombre(&bloque.cuerpo[anterior - 1]))
        {
            anterior -= 1;
            a_borrar.push(anterior);
        }
    }
    a_borrar.sort_unstable();
    a_borrar.dedup();
    for &posicion in a_borrar.iter().rev() {
        bloque.cuerpo.remove(posicion);
    }

    // El punto de inserción se corrige por lo que se haya borrado antes de él.
    let borradas_antes = a_borrar.iter().filter(|&&p| p < destino).count();
    let mut destino = destino
        .saturating_sub(borradas_antes)
        .min(bloque.cuerpo.len());

    for reenvio in reenvios {
        // El nombre va el primero: es lo que identifica al reenvío, y leerlo
        // antes que su comprobación es el orden natural.
        if let Some(nombre) = &reenvio.nombre {
            bloque.cuerpo.insert(
                destino,
                Linea::analizar(&format!("    # {} {}", Reenvio::MARCA_NOMBRE, nombre)),
            );
            destino += 1;
        }
        if reenvio.salud != Salud::Escucha {
            bloque.cuerpo.insert(
                destino,
                Linea::analizar(&format!(
                    "    # {} {}",
                    Salud::MARCA,
                    reenvio.salud.especificacion()
                )),
            );
            destino += 1;
        }
        bloque
            .cuerpo
            .insert(destino, Linea::directiva(clave, &reenvio.valor_directiva()));
        destino += 1;
    }
}

/// Índice donde insertar una directiva nueva: tras la última directiva
/// gestionada, o tras la última directiva cualquiera si no hay ninguna.
fn punto_de_insercion(bloque: &Bloque) -> usize {
    bloque
        .cuerpo
        .iter()
        .rposition(|l| {
            l.clave_normalizada()
                .map(|c| es_gestionada(&c))
                .unwrap_or(false)
        })
        .or_else(|| bloque.cuerpo.iter().rposition(|l| l.valor().is_some()))
        .map(|p| p + 1)
        .unwrap_or_else(|| {
            // Bloque sin directivas: tras la nota inicial, si la hay.
            usize::from(
                bloque
                    .cuerpo
                    .first()
                    .map(|l| l.clase == Clase::Comentario)
                    .unwrap_or(false),
            )
        })
}

fn fijar_unica(bloque: &mut Bloque, clave: &str, valor: Option<&str>) {
    let normalizada = clave.to_ascii_lowercase();
    let posiciones: Vec<usize> = bloque
        .cuerpo
        .iter()
        .enumerate()
        .filter(|(_, l)| l.clave_normalizada().as_deref() == Some(normalizada.as_str()))
        .map(|(i, _)| i)
        .collect();

    match valor.filter(|v| !v.trim().is_empty()) {
        Some(valor) => {
            let nueva = Linea::directiva(clave, valor);
            match posiciones.first() {
                Some(&primera) => {
                    bloque.cuerpo[primera] = nueva;
                    // Duplicados: OpenSSH ignora todos menos el primero, así
                    // que los sobrantes solo sirven para confundir.
                    for &p in posiciones.iter().skip(1).rev() {
                        bloque.cuerpo.remove(p);
                    }
                }
                None => {
                    let indice = punto_de_insercion(bloque);
                    bloque.cuerpo.insert(indice, nueva);
                }
            }
        }
        None => {
            for &p in posiciones.iter().rev() {
                bloque.cuerpo.remove(p);
            }
        }
    }
}

fn fijar_multiple(bloque: &mut Bloque, clave: &str, valores: &[String]) {
    let normalizada = clave.to_ascii_lowercase();
    let posiciones: Vec<usize> = bloque
        .cuerpo
        .iter()
        .enumerate()
        .filter(|(_, l)| l.clave_normalizada().as_deref() == Some(normalizada.as_str()))
        .map(|(i, _)| i)
        .collect();

    let destino = posiciones
        .first()
        .copied()
        .unwrap_or_else(|| punto_de_insercion(bloque));

    for &p in posiciones.iter().rev() {
        bloque.cuerpo.remove(p);
    }

    // El punto de inserción puede haberse desplazado al borrar.
    let destino = destino.min(bloque.cuerpo.len());
    for (desplazamiento, valor) in valores.iter().enumerate() {
        bloque
            .cuerpo
            .insert(destino + desplazamiento, Linea::directiva(clave, valor));
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;
    use crate::modelo::{Extremo, Origen, Reenvio, Salud};
    use std::path::PathBuf;

    fn documento(texto: &str) -> Documento {
        Documento::analizar(&PathBuf::from("/dev/null"), texto)
    }

    #[test]
    fn anadir_un_host_nuevo() {
        let mut doc = documento("# cabecera\n");
        let mut host = Host::nuevo("preprod");
        host.hostname = Some("host.interno".into());
        host.usuario = Some("usuario".into());
        host.reenvios.push(Reenvio::local(
            Extremo::solo_puerto(3000),
            Extremo::nuevo("localhost", 3000),
        ));
        sincronizar_host(&mut doc, &host).unwrap();

        let texto = doc.a_texto();
        assert!(texto.contains("Host preprod"));
        assert!(texto.contains("    HostName host.interno"));
        assert!(texto.contains("    LocalForward 3000 localhost:3000"));
        assert!(texto.contains("    ExitOnForwardFailure yes"));
        assert!(texto.starts_with("# cabecera"));
    }

    #[test]
    fn editar_preserva_comentarios_y_directivas_ajenas() {
        let original = "\
# nota: mi entorno
Host preprod
    HostName viejo.example
    # comentario en medio
    Compression yes
    LocalForward 3000 localhost:3000
    RequestTTY no
";
        let mut doc = documento(original);
        let mut host = doc
            .bloque("preprod")
            .unwrap()
            .a_host(Origen::Propio)
            .unwrap();
        host.hostname = Some("nuevo.example".into());
        host.reenvios.push(Reenvio::local(
            Extremo::solo_puerto(5432),
            Extremo::nuevo("db.interno", 5432),
        ));
        sincronizar_host(&mut doc, &host).unwrap();

        let texto = doc.a_texto();
        assert!(texto.contains("HostName nuevo.example"));
        assert!(!texto.contains("viejo.example"));
        assert!(texto.contains("# comentario en medio"));
        assert!(texto.contains("Compression yes"));
        assert!(texto.contains("RequestTTY no"));
        assert!(texto.contains("LocalForward 3000 localhost:3000"));
        assert!(texto.contains("LocalForward 5432 db.interno:5432"));
        assert!(texto.contains("# nota: mi entorno"));
    }

    #[test]
    fn quitar_un_reenvio_borra_solo_su_linea() {
        let mut doc = documento(
            "Host preprod\n    LocalForward 3000 localhost:3000\n    LocalForward 5432 db:5432\n    Compression yes\n",
        );
        let mut host = doc
            .bloque("preprod")
            .unwrap()
            .a_host(Origen::Propio)
            .unwrap();
        assert_eq!(host.reenvios.len(), 2);
        host.hostname = Some("h".into());
        host.reenvios.retain(|r| r.escucha.puerto != 3000);
        sincronizar_host(&mut doc, &host).unwrap();

        let texto = doc.a_texto();
        assert!(!texto.contains("LocalForward 3000"));
        assert!(texto.contains("LocalForward 5432 db:5432"));
        assert!(texto.contains("Compression yes"));
    }

    #[test]
    fn quitar_el_proxyjump_borra_la_directiva() {
        let mut doc = documento("Host p\n    HostName h\n    ProxyJump salto\n");
        let mut host = doc.bloque("p").unwrap().a_host(Origen::Propio).unwrap();
        host.saltos.clear();
        sincronizar_host(&mut doc, &host).unwrap();
        assert!(!doc.a_texto().contains("ProxyJump"));
    }

    #[test]
    fn renombrar_conserva_el_cuerpo() {
        let mut doc = documento("Host viejo\n    HostName h\n    Compression yes\n");
        renombrar_host(&mut doc, "viejo", "nuevo").unwrap();
        let texto = doc.a_texto();
        assert!(texto.contains("Host nuevo"));
        assert!(texto.contains("Compression yes"));
        assert!(!texto.contains("Host viejo"));
    }

    #[test]
    fn renombrar_a_uno_existente_falla() {
        let mut doc = documento("Host a\n    HostName h\n\nHost b\n    HostName h\n");
        assert!(matches!(
            renombrar_host(&mut doc, "a", "b"),
            Err(Error::HostDuplicado(_))
        ));
    }

    #[test]
    fn eliminar_borra_el_bloque_y_sus_comentarios() {
        let mut doc = documento("# nota: una\nHost a\n    HostName h\n\nHost b\n    HostName h\n");
        eliminar_host(&mut doc, "a").unwrap();
        let texto = doc.a_texto();
        assert!(!texto.contains("Host a"));
        assert!(!texto.contains("# nota: una"));
        assert!(texto.contains("Host b"));
    }

    #[test]
    fn los_duplicados_de_una_directiva_unica_se_colapsan() {
        let mut doc = documento("Host a\n    HostName uno\n    HostName dos\n");
        let mut host = doc.bloque("a").unwrap().a_host(Origen::Propio).unwrap();
        host.hostname = Some("tres".into());
        sincronizar_host(&mut doc, &host).unwrap();
        let texto = doc.a_texto();
        assert_eq!(texto.matches("HostName").count(), 1);
        assert!(texto.contains("HostName tres"));
    }

    #[test]
    fn el_nombre_va_y_vuelve_pegado_a_su_reenvio() {
        let mut doc = documento("");
        let mut host = Host::nuevo("salto");
        host.hostname = Some("192.0.2.10".into());
        host.reenvios.push(
            Reenvio::local(
                Extremo::solo_puerto(1522),
                Extremo::nuevo("192.0.2.30", 1521),
            )
            .con_nombre(Some("oracle-preprod".into())),
        );
        host.reenvios.push(
            Reenvio::local(
                Extremo::solo_puerto(3000),
                Extremo::nuevo("192.0.2.50", 3000),
            )
            .con_salud(Salud::Http {
                ruta: "/api/health".into(),
            })
            .con_nombre(Some("api-health".into())),
        );
        // Sin nombre: no debe escribirse marca ninguna.
        host.reenvios.push(Reenvio::local(
            Extremo::solo_puerto(2222),
            Extremo::nuevo("192.0.2.40", 22),
        ));
        sincronizar_host(&mut doc, &host).unwrap();

        let texto = doc.a_texto();
        assert!(texto.contains("    # nombre: oracle-preprod\n    LocalForward 1522"));
        // Con las dos marcas, el nombre va encima de la salud.
        assert!(texto.contains(
            "    # nombre: api-health\n    # salud: http:/api/health\n    LocalForward 3000"
        ));
        assert!(!texto.contains("nombre: puerto"));

        let vuelta = doc.bloque("salto").unwrap().a_host(Origen::Propio).unwrap();
        assert_eq!(vuelta.reenvios[0].nombre.as_deref(), Some("oracle-preprod"));
        assert_eq!(vuelta.reenvios[1].nombre.as_deref(), Some("api-health"));
        assert_eq!(vuelta.reenvios[2].nombre, None);
    }

    #[test]
    fn sin_nombre_propio_se_deriva_del_destino_y_nunca_del_alias() {
        let oracle = Reenvio::local(Extremo::solo_puerto(1522), Extremo::nuevo("interno", 1521));
        assert_eq!(oracle.nombre_visible(), "oracle-1521");
        let raro = Reenvio::local(Extremo::solo_puerto(9418), Extremo::nuevo("interno", 9418));
        assert_eq!(raro.nombre_visible(), "puerto-9418");
        // El nombre elegido manda sobre el derivado.
        assert_eq!(
            oracle
                .con_nombre(Some("  base-preprod  ".into()))
                .nombre_visible(),
            "base-preprod"
        );
    }

    #[test]
    fn la_marca_de_salud_va_y_vuelve_pegada_a_su_reenvio() {
        let mut doc = documento("");
        let mut host = Host::nuevo("salto");
        host.hostname = Some("192.0.2.10".into());
        host.reenvios.push(Reenvio::local(
            Extremo::solo_puerto(5001),
            Extremo::nuevo("192.0.2.20", 5001),
        ));
        host.reenvios.push(
            Reenvio::local(
                Extremo::solo_puerto(3000),
                Extremo::nuevo("192.0.2.50", 3000),
            )
            .con_salud(Salud::Http {
                ruta: "/api/health".into(),
            }),
        );
        host.reenvios.push(
            Reenvio::local(Extremo::solo_puerto(2222), Extremo::nuevo("192.0.2.40", 22))
                .con_salud(Salud::Banner),
        );
        sincronizar_host(&mut doc, &host).unwrap();

        let texto = doc.a_texto();
        // La marca va justo encima de su reenvío, no en cualquier sitio.
        assert!(texto.contains("    # salud: http:/api/health\n    LocalForward 3000"));
        assert!(texto.contains("    # salud: banner\n    LocalForward 2222"));
        // El reenvío sin salud especial no lleva marca.
        assert!(!texto.contains("# salud: escucha"));

        let vuelta = doc.bloque("salto").unwrap().a_host(Origen::Propio).unwrap();
        assert_eq!(vuelta.reenvios.len(), 3);
        assert_eq!(vuelta.reenvios[0].salud, Salud::Escucha);
        assert_eq!(
            vuelta.reenvios[1].salud,
            Salud::Http {
                ruta: "/api/health".into()
            }
        );
        assert_eq!(vuelta.reenvios[2].salud, Salud::Banner);
    }

    #[test]
    fn quitar_la_salud_borra_su_marca_y_no_deja_huerfanas() {
        let original = "Host a\n    HostName h\n    # salud: banner\n    LocalForward 22 x:22\n";
        let mut doc = documento(original);
        let mut host = doc.bloque("a").unwrap().a_host(Origen::Propio).unwrap();
        assert_eq!(host.reenvios[0].salud, Salud::Banner);

        host.reenvios[0].salud = Salud::Escucha;
        sincronizar_host(&mut doc, &host).unwrap();

        let texto = doc.a_texto();
        assert!(
            !texto.contains("salud:"),
            "quedó una marca huérfana:\n{texto}"
        );
        assert!(texto.contains("LocalForward 22 x:22"));
    }

    #[test]
    fn la_marca_no_se_pega_al_reenvio_equivocado_al_borrar_uno() {
        let original = "Host a\n    HostName h\n\
                        \x20   # salud: banner\n    LocalForward 22 x:22\n\
                        \x20   # salud: http:/ok\n    LocalForward 80 y:80\n";
        let mut doc = documento(original);
        let mut host = doc.bloque("a").unwrap().a_host(Origen::Propio).unwrap();
        host.reenvios.retain(|r| r.escucha.puerto != 22);
        sincronizar_host(&mut doc, &host).unwrap();

        let vuelta = doc.bloque("a").unwrap().a_host(Origen::Propio).unwrap();
        assert_eq!(vuelta.reenvios.len(), 1);
        assert_eq!(vuelta.reenvios[0].escucha.puerto, 80);
        assert_eq!(
            vuelta.reenvios[0].salud,
            Salud::Http { ruta: "/ok".into() },
            "el 80 se quedó con la salud del 22"
        );
        assert!(!doc.a_texto().contains("banner"));
    }

    #[test]
    fn se_escribe_tcpkeepalive_entre_las_directivas_de_robustez() {
        let mut doc = documento("");
        let mut host = Host::nuevo("a");
        host.hostname = Some("h".into());
        host.reenvios.push(Reenvio::local(
            Extremo::solo_puerto(80),
            Extremo::nuevo("x", 80),
        ));
        sincronizar_host(&mut doc, &host).unwrap();
        assert!(doc.a_texto().contains("TCPKeepAlive yes"));
    }

    #[test]
    fn la_ida_y_vuelta_es_estable() {
        let mut doc = documento("Host a\n    HostName h\n    LocalForward 80 web:80\n");
        let host = doc.bloque("a").unwrap().a_host(Origen::Propio).unwrap();
        sincronizar_host(&mut doc, &host).unwrap();
        let primera = doc.a_texto();
        let host = doc.bloque("a").unwrap().a_host(Origen::Propio).unwrap();
        sincronizar_host(&mut doc, &host).unwrap();
        assert_eq!(
            primera,
            doc.a_texto(),
            "sincronizar dos veces debe ser idempotente"
        );
    }
}
