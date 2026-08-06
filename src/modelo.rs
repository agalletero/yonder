//! Modelo de dominio: hosts, reenvíos y túneles.
//!
//! El modelo contempla los tres tipos de reenvío de OpenSSH aunque la interfaz
//! de la primera versión solo exponga el local (§9): el coste de modelarlos
//! bien ahora es nulo y el de añadirlos después no lo es.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Resultado};

/// Un extremo de un reenvío: dirección opcional más puerto.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Extremo {
    /// `None` significa el valor por defecto de OpenSSH (`localhost` para el
    /// lado de escucha, obligatorio para el lado de destino).
    pub direccion: Option<String>,
    pub puerto: u16,
}

impl Extremo {
    pub fn nuevo(direccion: impl Into<String>, puerto: u16) -> Self {
        Extremo {
            direccion: Some(direccion.into()),
            puerto,
        }
    }

    pub fn solo_puerto(puerto: u16) -> Self {
        Extremo {
            direccion: None,
            puerto,
        }
    }

    /// Dirección efectiva para sondear el puerto localmente.
    pub fn direccion_efectiva(&self) -> &str {
        self.direccion.as_deref().unwrap_or("127.0.0.1")
    }

    /// `true` si la escucha se abre a toda la red y no solo al bucle local.
    pub fn expuesto(&self) -> bool {
        matches!(
            self.direccion.as_deref(),
            Some("0.0.0.0") | Some("*") | Some("::")
        )
    }

    /// Analiza `puerto`, `host:puerto` o `[::1]:puerto`.
    fn analizar(texto: &str) -> Resultado<Extremo> {
        let texto = texto.trim();
        if texto.is_empty() {
            return Err(Error::DefinicionInvalida("extremo vacío".into()));
        }

        // Forma con corchetes para IPv6: [::1]:3000
        if let Some(resto) = texto.strip_prefix('[') {
            let (direccion, cola) = resto
                .split_once(']')
                .ok_or_else(|| Error::DefinicionInvalida(format!("falta «]» en «{texto}»")))?;
            let puerto = cola.trim_start_matches(':');
            return Ok(Extremo {
                direccion: Some(direccion.to_string()),
                puerto: analizar_puerto(puerto)?,
            });
        }

        match texto.rsplit_once(':') {
            Some((direccion, puerto)) if !direccion.is_empty() => Ok(Extremo {
                direccion: Some(direccion.to_string()),
                puerto: analizar_puerto(puerto)?,
            }),
            _ => Ok(Extremo {
                direccion: None,
                puerto: analizar_puerto(texto)?,
            }),
        }
    }
}

impl fmt::Display for Extremo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.direccion {
            Some(dir) if dir.contains(':') => write!(f, "[{dir}]:{}", self.puerto),
            Some(dir) => write!(f, "{dir}:{}", self.puerto),
            None => write!(f, "{}", self.puerto),
        }
    }
}

/// Entrecomilla un argumento si al pegarlo en una shell no sobreviviría entero.
///
/// El caso real es una ruta de clave con un espacio dentro. Se entrecomilla solo
/// cuando hace falta: una orden llena de comillas innecesarias se lee peor, y
/// esta está para copiarla y entenderla de un vistazo.
fn entrecomillar(argumento: &str) -> String {
    let seguro = !argumento.is_empty()
        && argumento
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.,:/@=+[]".contains(c));
    if seguro {
        argumento.to_string()
    } else {
        format!("'{}'", argumento.replace('\'', r"'\''"))
    }
}

/// Rechaza saltos de línea y caracteres de control.
///
/// Un valor con `\n` dentro se convertiría en directivas nuevas al escribirse
/// en `yonder.conf` —una `ProxyCommand` colada en el campo nota ejecutaría
/// órdenes en la siguiente conexión—. El resto de caracteres de control no
/// abren directivas, pero tampoco pintan nada en una configuración.
fn sin_control(campo: &str, valor: &str) -> Resultado<()> {
    if valor.chars().any(char::is_control) {
        return Err(Error::DefinicionInvalida(format!(
            "{campo} no puede contener saltos de línea ni caracteres de control"
        )));
    }
    Ok(())
}

/// Rechaza espacios donde `ssh_config` tomaría solo la primera palabra.
fn sin_espacios(campo: &str, valor: &str) -> Resultado<()> {
    if valor.contains(char::is_whitespace) {
        return Err(Error::DefinicionInvalida(format!(
            "{campo} no puede contener espacios"
        )));
    }
    Ok(())
}

/// Rechaza valores que `ssh` consumiría como opción.
///
/// Todo lo que sale hacia un proceso va como argumento separado, pero `ssh`,
/// `ssh-keyscan` y `ssh-copy-id` no admiten el separador `--`: un destino
/// `-oProxyCommand=…` se interpretaría como opción de todos modos.
fn sin_guion_inicial(campo: &str, valor: &str) -> Resultado<()> {
    if valor.starts_with('-') {
        return Err(Error::DefinicionInvalida(format!(
            "{campo} no puede empezar por «-»: ssh lo tomaría por una opción"
        )));
    }
    Ok(())
}

fn analizar_puerto(texto: &str) -> Resultado<u16> {
    texto
        .trim()
        .parse::<u16>()
        .map_err(|_| Error::DefinicionInvalida(format!("«{texto}» no es un puerto válido")))
        .and_then(|p| {
            if p == 0 {
                Err(Error::DefinicionInvalida("el puerto 0 no es válido".into()))
            } else {
                Ok(p)
            }
        })
}

/// Tipo de reenvío. Se corresponde uno a uno con las directivas de OpenSSH.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum TipoReenvio {
    /// `LocalForward` / `-L`: un puerto local se reenvía al lado remoto.
    ///
    /// Es el caso por defecto: el 90 % de los túneles son esto.
    #[default]
    Local,
    /// `RemoteForward` / `-R`: un puerto remoto se reenvía al lado local.
    Remoto,
    /// `DynamicForward` / `-D`: proxy SOCKS local.
    Dinamico,
}

impl TipoReenvio {
    pub fn directiva(&self) -> &'static str {
        match self {
            TipoReenvio::Local => "LocalForward",
            TipoReenvio::Remoto => "RemoteForward",
            TipoReenvio::Dinamico => "DynamicForward",
        }
    }

    pub fn bandera(&self) -> &'static str {
        match self {
            TipoReenvio::Local => "-L",
            TipoReenvio::Remoto => "-R",
            TipoReenvio::Dinamico => "-D",
        }
    }

    pub fn etiqueta(&self) -> &'static str {
        match self {
            TipoReenvio::Local => "local",
            TipoReenvio::Remoto => "remoto",
            TipoReenvio::Dinamico => "SOCKS",
        }
    }

    /// `true` si el puerto de escucha se abre en esta máquina.
    ///
    /// Determina si tiene sentido sondear el puerto localmente para detectar el
    /// estado `Degradado` (§4).
    pub fn escucha_en_local(&self) -> bool {
        matches!(self, TipoReenvio::Local | TipoReenvio::Dinamico)
    }
}

/// Cómo se comprueba que un túnel está **de verdad** en pie.
///
/// Que el puerto local escuche no demuestra nada: el reenvío puede estar
/// establecido y no llegar ni una petición al otro extremo. Pasa, por ejemplo,
/// cuando el destino es `localhost` y el servicio remoto está enganchado a un
/// alias de IP concreto: `ssh` acepta el reenvío, el puerto local abre, y las
/// conexiones mueren al otro lado. Un panel que solo mire la escucha enseña un
/// punto verde sobre un túnel muerto, que es exactamente lo que §4 quiere
/// evitar con el estado `Degradado`.
///
/// Se declara en el propio `ssh_config` con un comentario delante del reenvío:
///
/// ```text
///     # salud: http:/api/health
///     LocalForward 3000 192.0.2.50:3000
/// ```
///
/// `ssh` lo ignora por ser un comentario, así que la definición sigue viviendo
/// en un único sitio (§3.1) y `ssh <alias>` no se entera de nada.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Salud {
    /// Basta con que el puerto local esté en escucha. Es lo más barato y lo
    /// único que se puede hacer sin tocar el servicio del otro lado.
    #[default]
    Escucha,
    /// Abrir una conexión TCP y cerrarla. Detecta que el reenvío llega a algún
    /// sitio, sin llegar a hablar con el servicio.
    Conecta,
    /// Abrir la conexión y esperar a que el servidor hable primero. Sirve para
    /// SSH y Oracle, que saludan al conectar.
    Banner,
    /// Petición HTTP a `http://localhost:<puerto><ruta>`; vale cualquier
    /// respuesta que no sea error del servidor.
    Http { ruta: String },
}

impl Salud {
    /// Marca con la que se declara en `ssh_config`.
    pub const MARCA: &'static str = "salud:";

    /// Analiza la especificación tal como se escribe tras `# salud:`.
    pub fn analizar(texto: &str) -> Resultado<Salud> {
        let texto = texto.trim();
        if let Some(ruta) = texto.strip_prefix("http:") {
            let ruta = ruta.trim();
            if !ruta.starts_with('/') {
                return Err(Error::DefinicionInvalida(format!(
                    "la ruta de «http:» debe empezar por «/»; se recibió «{ruta}»"
                )));
            }
            return Ok(Salud::Http {
                ruta: ruta.to_string(),
            });
        }
        match texto.to_ascii_lowercase().as_str() {
            "" | "escucha" | "listen" => Ok(Salud::Escucha),
            "tcp" | "conecta" => Ok(Salud::Conecta),
            "banner" => Ok(Salud::Banner),
            otro => Err(Error::DefinicionInvalida(format!(
                "«{otro}» no es una comprobación de salud conocida; \
                 usa escucha, tcp, banner o http:/ruta"
            ))),
        }
    }

    /// Especificación tal como se escribe en el fichero.
    pub fn especificacion(&self) -> String {
        match self {
            Salud::Escucha => "escucha".to_string(),
            Salud::Conecta => "tcp".to_string(),
            Salud::Banner => "banner".to_string(),
            Salud::Http { ruta } => format!("http:{ruta}"),
        }
    }

    /// Texto corto para la interfaz.
    pub fn etiqueta(&self) -> String {
        match self {
            Salud::Escucha => "escucha".to_string(),
            Salud::Conecta => "TCP".to_string(),
            Salud::Banner => "banner".to_string(),
            Salud::Http { ruta } => format!("HTTP {ruta}"),
        }
    }

    /// Explicación de qué comprueba, para la ayuda contextual.
    pub fn explicacion(&self) -> &'static str {
        match self {
            Salud::Escucha => "Solo comprueba que el puerto local esté abierto",
            Salud::Conecta => "Abre una conexión TCP a través del túnel y la cierra",
            Salud::Banner => "Espera a que el servidor salude, como hacen SSH y Oracle",
            Salud::Http { .. } => "Hace una petición HTTP y comprueba la respuesta",
        }
    }

    /// `true` si la comprobación atraviesa el túnel de verdad.
    ///
    /// Las que lo atraviesan detectan el zombi, pero tienen efecto al otro
    /// lado: son una conexión real. Por eso se ejecutan con su propia cadencia,
    /// más lenta que el latido del supervisor.
    pub fn atraviesa_el_tunel(&self) -> bool {
        !matches!(self, Salud::Escucha)
    }

    /// Comprobaciones que la interfaz ofrece para elegir.
    pub fn opciones() -> Vec<Salud> {
        vec![
            Salud::Escucha,
            Salud::Conecta,
            Salud::Banner,
            Salud::Http {
                ruta: "/".to_string(),
            },
        ]
    }

    /// Igualdad por variante, ignorando la ruta. Lo usa el selector.
    pub fn misma_clase(&self, otra: &Salud) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(otra)
    }
}

/// Nombre corto del servicio que suele escuchar en un puerto.
///
/// Solo los que aparecen de verdad en los túneles de trabajo. No pretende ser
/// la lista de IANA: un nombre inventado para un puerto poco común confunde
/// más que un «puerto-9418» honesto.
pub fn servicio_de(puerto: u16) -> Option<&'static str> {
    Some(match puerto {
        22 => "ssh",
        80 => "http",
        443 => "https",
        1521 => "oracle",
        3000 | 3001 => "grafana",
        3306 => "mysql",
        5432 => "postgres",
        6379 => "redis",
        8080 | 8081 => "web",
        9090 => "prometheus",
        9093 => "alertmanager",
        27017 => "mongo",
        _ => return None,
    })
}

/// Un reenvío concreto.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Reenvio {
    pub tipo: TipoReenvio,
    pub escucha: Extremo,
    /// `None` solo para `DynamicForward`.
    pub destino: Option<Extremo>,
    /// Cómo comprobar que está vivo de verdad.
    pub salud: Salud,
    /// Nombre corto elegido por el usuario. Es lo que manda en la lista.
    ///
    /// Sin él, cada fila tenía que encabezarse con el alias del host, que es el
    /// mismo para todos sus reenvíos: siete filas cuyo elemento más prominente
    /// era la única palabra que no distinguía ninguna.
    ///
    /// Se persiste como comentario `# nombre:` delante de su directiva, igual
    /// que `# salud:`, para no salirse de la gramática de `ssh_config`.
    pub nombre: Option<String>,
}

impl Reenvio {
    pub fn local(escucha: Extremo, destino: Extremo) -> Self {
        Reenvio {
            tipo: TipoReenvio::Local,
            escucha,
            destino: Some(destino),
            salud: Salud::default(),
            nombre: None,
        }
    }

    pub fn dinamico(escucha: Extremo) -> Self {
        Reenvio {
            tipo: TipoReenvio::Dinamico,
            escucha,
            destino: None,
            salud: Salud::default(),
            nombre: None,
        }
    }

    /// El mismo reenvío con otra comprobación de salud.
    pub fn con_salud(mut self, salud: Salud) -> Self {
        self.salud = salud;
        self
    }

    /// El mismo reenvío con nombre propio.
    pub fn con_nombre(mut self, nombre: Option<String>) -> Self {
        self.nombre = nombre
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty());
        self
    }

    /// Marca con la que el nombre se persiste en `ssh_config`.
    pub const MARCA_NOMBRE: &'static str = "nombre:";

    /// Extremos en su forma más corta discriminante, para la lista.
    ///
    /// `127.0.0.1` es el valor por defecto del lado de escucha y es idéntico en
    /// todas las filas: ocupa sitio y no distingue nada, así que se omite y el
    /// `:` inicial basta para decir que es el puerto local. La dirección solo
    /// aparece cuando NO es el bucle local, porque entonces sí es un hecho
    /// relevante: el puerto está abierto a la red.
    ///
    /// Con esta forma sobran las palabras «aquí» y «remoto» que llevaba antes:
    /// la asimetría entre `:1522` y un destino completo ya lo dice.
    pub fn extremos_breves(&self) -> String {
        let escucha = match &self.escucha.direccion {
            Some(dir) if dir != "127.0.0.1" && dir != "localhost" => {
                format!("{dir}:{}", self.escucha.puerto)
            }
            _ => format!(":{}", self.escucha.puerto),
        };
        match (&self.tipo, &self.destino) {
            (TipoReenvio::Dinamico, _) => format!("{escucha} → SOCKS"),
            (TipoReenvio::Remoto, Some(destino)) => format!("remoto {escucha} → {destino}"),
            (_, Some(destino)) => format!("{escucha} → {destino}"),
            (_, None) => escucha,
        }
    }

    /// Nombre que se muestra en la lista.
    ///
    /// El elegido por el usuario si lo hay; si no, uno derivado del **destino**,
    /// nunca del alias del host: el alias ya encabeza el grupo y repetirlo en
    /// cada fila es lo que hacía la lista ilegible.
    pub fn nombre_visible(&self) -> String {
        if let Some(nombre) = &self.nombre {
            return nombre.clone();
        }
        match (&self.tipo, &self.destino) {
            (TipoReenvio::Dinamico, _) => format!("socks-{}", self.escucha.puerto),
            (_, Some(destino)) => match servicio_de(destino.puerto) {
                Some(servicio) => format!("{servicio}-{}", destino.puerto),
                None => format!("puerto-{}", destino.puerto),
            },
            (_, None) => format!("puerto-{}", self.escucha.puerto),
        }
    }

    /// Analiza el valor de una directiva `*Forward` de `ssh_config`.
    ///
    /// OpenSSH admite `:` y `/` como separador; se aceptan ambos.
    pub fn analizar(tipo: TipoReenvio, valor: &str) -> Resultado<Reenvio> {
        let valor = valor.replace('/', ":");
        let campos: Vec<&str> = valor.split_whitespace().collect();

        match (tipo, campos.as_slice()) {
            (TipoReenvio::Dinamico, [escucha]) => Ok(Reenvio {
                tipo,
                escucha: Extremo::analizar(escucha)?,
                destino: None,
                salud: Salud::default(),
                nombre: None,
            }),
            (TipoReenvio::Local | TipoReenvio::Remoto, [escucha, destino]) => Ok(Reenvio {
                tipo,
                escucha: Extremo::analizar(escucha)?,
                destino: Some(Extremo::analizar(destino)?),
                salud: Salud::default(),
                nombre: None,
            }),
            // Forma compacta de una sola palabra: 3000:localhost:3000
            (TipoReenvio::Local | TipoReenvio::Remoto, [unico]) => {
                let partes: Vec<&str> = unico.split(':').collect();
                match partes.as_slice() {
                    [escucha, host, puerto] => Ok(Reenvio {
                        tipo,
                        escucha: Extremo::analizar(escucha)?,
                        destino: Some(Extremo::nuevo(*host, analizar_puerto(puerto)?)),
                        salud: Salud::default(),
                        nombre: None,
                    }),
                    [enlace, escucha, host, puerto] => Ok(Reenvio {
                        tipo,
                        escucha: Extremo::nuevo(*enlace, analizar_puerto(escucha)?),
                        destino: Some(Extremo::nuevo(*host, analizar_puerto(puerto)?)),
                        salud: Salud::default(),
                        nombre: None,
                    }),
                    _ => Err(Error::DefinicionInvalida(format!(
                        "«{}» no es un valor válido para {}",
                        valor,
                        tipo.directiva()
                    ))),
                }
            }
            _ => Err(Error::DefinicionInvalida(format!(
                "«{}» no es un valor válido para {}",
                valor,
                tipo.directiva()
            ))),
        }
    }

    /// Valor tal como se escribe en `ssh_config`.
    pub fn valor_directiva(&self) -> String {
        match &self.destino {
            Some(destino) => format!("{} {}", self.escucha, destino),
            None => self.escucha.to_string(),
        }
    }

    /// Argumento para `-L` / `-R` / `-D` en la línea de órdenes de `ssh`.
    ///
    /// Aquí el separador debe ser `:` sin espacios, que es lo que espera
    /// `-O forward`.
    pub fn argumento(&self) -> String {
        let escucha = match &self.escucha.direccion {
            Some(dir) => format!("{dir}:{}", self.escucha.puerto),
            None => self.escucha.puerto.to_string(),
        };
        match &self.destino {
            Some(destino) => format!(
                "{escucha}:{}:{}",
                destino.direccion.as_deref().unwrap_or("localhost"),
                destino.puerto
            ),
            None => escucha,
        }
    }

    /// Descripción legible para la interfaz: `localhost:3000 → host:3000`.
    pub fn descripcion(&self) -> String {
        match (&self.tipo, &self.destino) {
            (TipoReenvio::Local, Some(destino)) => format!(
                "{}:{} → {}",
                self.escucha.direccion_efectiva(),
                self.escucha.puerto,
                destino
            ),
            (TipoReenvio::Remoto, Some(destino)) => {
                format!("remoto:{} → {}", self.escucha.puerto, destino)
            }
            (TipoReenvio::Dinamico, _) => format!(
                "SOCKS en {}:{}",
                self.escucha.direccion_efectiva(),
                self.escucha.puerto
            ),
            (_, None) => self.escucha.to_string(),
        }
    }

    /// Como `descripcion`, pero nombrando en qué máquina se resuelve cada punta.
    ///
    /// «127.0.0.1:3001 → localhost:3001» se lee mal: los dos parecen este
    /// equipo, cuando el de la derecha lo resuelve el servidor del otro lado.
    /// Con un destino explícito la ambigüedad no existe; con `localhost` es
    /// total, y es justo el caso en que equivocarse sale caro —un reenvío a
    /// `localhost` cuando el servicio remoto escucha en un alias de IP deja el
    /// puerto abierto sin transportar nada.
    ///
    /// No se sustituye `localhost` por el nombre del host, que sería lo cómodo:
    /// diría que ese puerto es alcanzable en esa dirección, y muchas veces no
    /// lo es. Se conserva el valor literal del fichero y se añade de quién es.
    ///
    /// Vive aparte de `descripcion` a propósito: esa la consumen el registro y
    /// el buscador, y meterle estas dos palabras haría que teclear «remoto»
    /// encontrara todos los túneles.
    pub fn descripcion_orientada(&self) -> String {
        match (&self.tipo, &self.destino) {
            (TipoReenvio::Local, Some(destino)) => format!(
                "aquí {}:{} → remoto {}",
                self.escucha.direccion_efectiva(),
                self.escucha.puerto,
                destino
            ),
            // En un reenvío remoto las puntas están cambiadas: el puerto lo abre
            // el servidor y el destino lo resuelve este equipo.
            (TipoReenvio::Remoto, Some(destino)) => {
                format!("remoto :{} → aquí {}", self.escucha.puerto, destino)
            }
            (TipoReenvio::Dinamico, _) => format!(
                "SOCKS aquí en {}:{}",
                self.escucha.direccion_efectiva(),
                self.escucha.puerto
            ),
            (_, None) => self.escucha.to_string(),
        }
    }
}

/// De dónde sale la definición de un host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origen {
    /// `~/.ssh/config.d/yonder.conf`: la aplicación es dueña y puede escribir.
    Propio,
    /// Otro fichero del usuario: se muestra y se puede activar, pero no editar.
    Ajeno(PathBuf),
}

impl Origen {
    pub fn editable(&self) -> bool {
        matches!(self, Origen::Propio)
    }
}

/// Un bloque `Host` con las directivas que la aplicación entiende.
///
/// Las directivas que no reconoce se conservan íntegras en el cuerpo del bloque
/// (véase `config::parser`), de modo que reescribir el fichero nunca pierde
/// información.
#[derive(Debug, Clone)]
pub struct Host {
    pub alias: String,
    pub hostname: Option<String>,
    pub usuario: Option<String>,
    pub puerto: Option<u16>,
    /// `ProxyJump`: uno o varios saltos separados por comas.
    pub saltos: Vec<String>,
    pub identidades: Vec<String>,
    pub reenvios: Vec<Reenvio>,
    pub origen: Origen,
    /// Comentario libre asociado al bloque, que la interfaz muestra como
    /// descripción. Se guarda en el fichero como `# nota: …`.
    pub nota: Option<String>,
}

impl Host {
    pub fn nuevo(alias: impl Into<String>) -> Self {
        Host {
            alias: alias.into(),
            hostname: None,
            usuario: None,
            puerto: None,
            saltos: Vec::new(),
            identidades: Vec::new(),
            reenvios: Vec::new(),
            origen: Origen::Propio,
            nota: None,
        }
    }

    /// Destino efectivo: `HostName` si existe, si no el propio alias.
    pub fn destino(&self) -> &str {
        self.hostname.as_deref().unwrap_or(&self.alias)
    }

    /// Destino con usuario y puerto, para mostrar: `usuario@host.interno:22`.
    pub fn destino_completo(&self) -> String {
        let mut texto = String::new();
        if let Some(usuario) = &self.usuario {
            texto.push_str(usuario);
            texto.push('@');
        }
        texto.push_str(self.destino());
        if let Some(puerto) = self.puerto {
            if puerto != 22 {
                texto.push(':');
                texto.push_str(&puerto.to_string());
            }
        }
        texto
    }

    /// `true` si alguna `IdentityFile` apunta a una clave respaldada por
    /// hardware (§3.5). Solo hay que **detectarlo** y avisar; el trabajo real lo
    /// hace el binario `ssh`.
    pub fn usa_clave_hardware(&self) -> bool {
        self.identidades.iter().any(|ruta| {
            let nombre = ruta.rsplit('/').next().unwrap_or(ruta);
            nombre.contains("_sk") || nombre.contains("-sk") || nombre.ends_with("sk")
        })
    }

    /// Túneles derivados de este host, uno por reenvío.
    pub fn tuneles(&self) -> Vec<Tunel> {
        self.reenvios
            .iter()
            .map(|reenvio| Tunel {
                alias: self.alias.clone(),
                reenvio: reenvio.clone(),
            })
            .collect()
    }

    /// La orden `ssh` equivalente a un reenvío, para ejecutarla a mano.
    ///
    /// **No** es la orden que ejecuta la aplicación. Esa abre un maestro con
    /// `-M -f -S <socket> -o ControlPersist=yes -o ClearAllForwardings=yes` y
    /// termina en el alias, porque después añade cada reenvío en caliente con
    /// `-O forward`. Copiar aquello no le serviría a nadie: depende de un
    /// socket de control de esta máquina y de que el alias exista en el
    /// `~/.ssh/config` de quien la pegue, que es precisamente lo que no tendrá
    /// si está en un servidor ajeno.
    ///
    /// Lo que se devuelve es una conexión puntual y autocontenida: todo lo que
    /// el bloque `Host` aportaría —puerto, saltos, clave, usuario— va escrito
    /// en la propia línea. Se sostiene sola en una máquina que no ha visto este
    /// fichero de configuración nunca.
    ///
    /// Queda fuera todo lo que es andamiaje de la aplicación: el multiplexado,
    /// los latidos, las sondas de salud. Quien copie esto quiere un túnel
    /// ahora, no la supervisión.
    ///
    /// Sin `-f`: en primer plano se ve si la conexión ha funcionado y se cierra
    /// con Ctrl-C. Mandarla al fondo en una terminal ajena deja un proceso que
    /// luego hay que buscar.
    pub fn orden_ssh_manual(&self, reenvio: &Reenvio) -> String {
        let mut partes = vec!["ssh".to_string(), "-N".to_string()];

        partes.push(reenvio.tipo.bandera().to_string());
        partes.push(entrecomillar(&reenvio.argumento()));

        if let Some(puerto) = self.puerto {
            if puerto != 22 {
                partes.push("-p".to_string());
                partes.push(puerto.to_string());
            }
        }

        // `-J` es la forma de línea de órdenes de ProxyJump, y encadena igual.
        if !self.saltos.is_empty() {
            partes.push("-J".to_string());
            partes.push(entrecomillar(&self.saltos.join(",")));
        }

        for identidad in &self.identidades {
            partes.push("-i".to_string());
            partes.push(entrecomillar(identidad));
        }

        let destino = match &self.usuario {
            Some(usuario) => format!("{usuario}@{}", self.destino()),
            None => self.destino().to_string(),
        };
        partes.push(entrecomillar(&destino));

        partes.join(" ")
    }

    /// Comprueba que la definición es coherente antes de guardarla.
    ///
    /// Además de la coherencia, aquí se cierra la puerta a dos abusos que la
    /// gramática permitiría: un valor con salto de línea se convertiría en
    /// **directivas nuevas** al escribirse en `yonder.conf` —incluida una
    /// `ProxyCommand`, que ejecuta órdenes—, y un valor que empieza por «-» lo
    /// consumiría `ssh` como opción, porque `ssh` no admite el separador `--`.
    /// El fichero es del propio usuario, pero «pega esto en el campo» es un
    /// vector real y la validación cuesta cuatro líneas.
    pub fn validar(&self) -> Resultado<()> {
        if self.alias.trim().is_empty() {
            return Err(Error::DefinicionInvalida(
                "el alias no puede estar vacío".into(),
            ));
        }
        if self.alias.contains(char::is_whitespace) {
            return Err(Error::DefinicionInvalida(
                "el alias no puede contener espacios".into(),
            ));
        }
        if self.alias.contains(['*', '?', '!']) {
            return Err(Error::DefinicionInvalida(
                "el alias no puede contener comodines: sería un patrón, no un host concreto".into(),
            ));
        }
        if self.destino().trim().is_empty() {
            return Err(Error::DefinicionInvalida(
                "hay que indicar un HostName o un alias que resuelva".into(),
            ));
        }

        sin_control("el alias", &self.alias)?;
        sin_guion_inicial("el alias", &self.alias)?;
        for (campo, valor) in [
            ("el HostName", &self.hostname),
            ("el usuario", &self.usuario),
        ] {
            if let Some(valor) = valor {
                sin_control(campo, valor)?;
                sin_espacios(campo, valor)?;
                sin_guion_inicial(campo, valor)?;
            }
        }
        for salto in &self.saltos {
            sin_control("un salto", salto)?;
            sin_espacios("un salto", salto)?;
            sin_guion_inicial("un salto", salto)?;
        }
        for identidad in &self.identidades {
            // Una ruta puede llevar espacios; lo que no puede llevar es un
            // salto de línea que abra una directiva nueva.
            sin_control("una IdentityFile", identidad)?;
        }
        if let Some(nota) = &self.nota {
            sin_control("la nota", nota)?;
        }
        for reenvio in &self.reenvios {
            if let Some(nombre) = &reenvio.nombre {
                sin_control("el nombre del reenvío", nombre)?;
            }
            if let Salud::Http { ruta } = &reenvio.salud {
                sin_control("la ruta de salud", ruta)?;
                sin_espacios("la ruta de salud", ruta)?;
            }
            for extremo in std::iter::once(&reenvio.escucha).chain(reenvio.destino.as_ref()) {
                if let Some(direccion) = &extremo.direccion {
                    sin_control("una dirección", direccion)?;
                    sin_espacios("una dirección", direccion)?;
                }
            }
        }
        for reenvio in &self.reenvios {
            if reenvio.tipo != TipoReenvio::Dinamico && reenvio.destino.is_none() {
                return Err(Error::DefinicionInvalida(format!(
                    "el reenvío {} necesita un destino",
                    reenvio.tipo.directiva()
                )));
            }
        }
        let mut escuchas: Vec<String> = self
            .reenvios
            .iter()
            .filter(|r| r.tipo.escucha_en_local())
            .map(|r| r.escucha.to_string())
            .collect();
        escuchas.sort();
        let total = escuchas.len();
        escuchas.dedup();
        if escuchas.len() != total {
            return Err(Error::DefinicionInvalida(
                "hay dos reenvíos escuchando en el mismo puerto local".into(),
            ));
        }
        Ok(())
    }
}

/// Unidad de gestión de la interfaz: un reenvío concreto sobre un alias.
///
/// El estado se lleva **por túnel**, no por host: un host puede tener varios
/// reenvíos y cada uno se activa y se cae por separado (§4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tunel {
    pub alias: String,
    pub reenvio: Reenvio,
}

impl Tunel {
    /// Identificador estable. El lado de escucha es único por definición: dos
    /// túneles no pueden escuchar en el mismo sitio.
    pub fn id(&self) -> String {
        format!(
            "{}|{}|{}",
            self.alias,
            self.reenvio.tipo.bandera(),
            self.reenvio.escucha
        )
    }

    pub fn nombre(&self) -> String {
        format!("{} · {}", self.alias, self.reenvio.descripcion())
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn analiza_reenvio_local_en_dos_campos() {
        let r = Reenvio::analizar(TipoReenvio::Local, "3000 localhost:3000").unwrap();
        assert_eq!(r.escucha, Extremo::solo_puerto(3000));
        assert_eq!(r.destino, Some(Extremo::nuevo("localhost", 3000)));
        assert_eq!(r.argumento(), "3000:localhost:3000");
    }

    #[test]
    fn analiza_reenvio_local_con_direccion_de_escucha() {
        let r = Reenvio::analizar(TipoReenvio::Local, "127.0.0.1:8080 interno:80").unwrap();
        assert_eq!(r.escucha, Extremo::nuevo("127.0.0.1", 8080));
        assert_eq!(r.argumento(), "127.0.0.1:8080:interno:80");
    }

    fn host_de_ejemplo() -> Host {
        let mut host = Host::nuevo("salto");
        host.hostname = Some("192.0.2.10".into());
        host.usuario = Some("usuario".into());
        host
    }

    #[test]
    fn la_orden_manual_se_sostiene_sin_el_fichero_de_configuracion() {
        let host = host_de_ejemplo();
        let reenvio = Reenvio::analizar(TipoReenvio::Local, "3001 localhost:3001").unwrap();
        // Ni el alias ni el socket de control: eso no existe en la otra máquina.
        assert_eq!(
            host.orden_ssh_manual(&reenvio),
            "ssh -N -L 3001:localhost:3001 usuario@192.0.2.10"
        );
    }

    #[test]
    fn la_orden_manual_no_arrastra_el_andamiaje_de_la_aplicacion() {
        let host = host_de_ejemplo();
        let reenvio = Reenvio::analizar(TipoReenvio::Local, "3001 localhost:3001").unwrap();
        let orden = host.orden_ssh_manual(&reenvio);
        for sobra in [
            "-M",
            "-S",
            "ControlPersist",
            "ClearAllForwardings",
            "ServerAlive",
            "salud",
        ] {
            assert!(
                !orden.contains(sobra),
                "«{sobra}» no pinta nada en «{orden}»"
            );
        }
    }

    #[test]
    fn la_orden_manual_lleva_puerto_saltos_y_clave() {
        let mut host = host_de_ejemplo();
        host.puerto = Some(2222);
        host.saltos = vec!["primero".into(), "segundo".into()];
        host.identidades = vec!["/home/u/.ssh/id_ed25519".into()];
        let reenvio = Reenvio::analizar(TipoReenvio::Local, "8080 interno:80").unwrap();
        assert_eq!(
            host.orden_ssh_manual(&reenvio),
            "ssh -N -L 8080:interno:80 -p 2222 -J primero,segundo \
             -i /home/u/.ssh/id_ed25519 usuario@192.0.2.10"
        );
    }

    #[test]
    fn el_puerto_22_no_se_escribe_por_ser_el_de_por_defecto() {
        let mut host = host_de_ejemplo();
        host.puerto = Some(22);
        let reenvio = Reenvio::analizar(TipoReenvio::Local, "3001 localhost:3001").unwrap();
        assert!(!host.orden_ssh_manual(&reenvio).contains("-p"));
    }

    #[test]
    fn la_orden_manual_entrecomilla_lo_que_la_shell_partiria() {
        let mut host = host_de_ejemplo();
        host.identidades = vec!["/home/u/mis claves/id_ed25519".into()];
        let reenvio = Reenvio::analizar(TipoReenvio::Local, "3001 localhost:3001").unwrap();
        assert!(host
            .orden_ssh_manual(&reenvio)
            .contains("-i '/home/u/mis claves/id_ed25519'"));
    }

    #[test]
    fn la_orden_manual_usa_la_bandera_de_cada_tipo() {
        let host = host_de_ejemplo();
        let remoto = Reenvio::analizar(TipoReenvio::Remoto, "8080 localhost:3000").unwrap();
        assert!(host
            .orden_ssh_manual(&remoto)
            .contains("-R 8080:localhost:3000"));
        let socks = Reenvio::analizar(TipoReenvio::Dinamico, "1080").unwrap();
        assert!(host.orden_ssh_manual(&socks).contains("-D 1080"));
    }

    #[test]
    fn sin_usuario_la_orden_lleva_solo_el_host() {
        let mut host = host_de_ejemplo();
        host.usuario = None;
        let reenvio = Reenvio::analizar(TipoReenvio::Local, "3001 localhost:3001").unwrap();
        assert!(host.orden_ssh_manual(&reenvio).ends_with(" 192.0.2.10"));
    }

    #[test]
    fn la_descripcion_orientada_dice_de_quien_es_cada_localhost() {
        let r = Reenvio::analizar(TipoReenvio::Local, "3001 localhost:3001").unwrap();
        // Sin orientar, las dos puntas parecen la misma máquina.
        assert_eq!(r.descripcion(), "127.0.0.1:3001 → localhost:3001");
        assert_eq!(
            r.descripcion_orientada(),
            "aquí 127.0.0.1:3001 → remoto localhost:3001"
        );
    }

    #[test]
    fn la_descripcion_orientada_conserva_el_destino_literal() {
        // No se sustituye por el nombre del host: diría que ese puerto es
        // alcanzable en esa dirección, y con un alias de IP no tiene por qué.
        let r = Reenvio::analizar(TipoReenvio::Local, "3000 192.0.2.50:3000").unwrap();
        assert_eq!(
            r.descripcion_orientada(),
            "aquí 127.0.0.1:3000 → remoto 192.0.2.50:3000"
        );
    }

    #[test]
    fn un_reenvio_remoto_tiene_las_puntas_cambiadas() {
        let r = Reenvio::analizar(TipoReenvio::Remoto, "8080 localhost:3000").unwrap();
        assert_eq!(
            r.descripcion_orientada(),
            "remoto :8080 → aquí localhost:3000"
        );
    }

    #[test]
    fn analiza_forma_compacta() {
        let r = Reenvio::analizar(TipoReenvio::Local, "3000:localhost:3000").unwrap();
        assert_eq!(r.escucha.puerto, 3000);
        assert_eq!(r.destino.as_ref().unwrap().puerto, 3000);
    }

    #[test]
    fn analiza_ipv6_entre_corchetes() {
        let r = Reenvio::analizar(TipoReenvio::Local, "[::1]:5432 [fd00::1]:5432").unwrap();
        assert_eq!(r.escucha.direccion.as_deref(), Some("::1"));
        assert_eq!(
            r.destino.as_ref().unwrap().direccion.as_deref(),
            Some("fd00::1")
        );
        assert_eq!(r.valor_directiva(), "[::1]:5432 [fd00::1]:5432");
    }

    #[test]
    fn analiza_dinamico() {
        let r = Reenvio::analizar(TipoReenvio::Dinamico, "1080").unwrap();
        assert_eq!(r.tipo, TipoReenvio::Dinamico);
        assert!(r.destino.is_none());
        assert_eq!(r.argumento(), "1080");
    }

    #[test]
    fn acepta_barra_como_separador() {
        let r = Reenvio::analizar(TipoReenvio::Local, "3000/localhost/3000").unwrap();
        assert_eq!(r.escucha.puerto, 3000);
    }

    #[test]
    fn rechaza_puerto_cero_y_basura() {
        assert!(Reenvio::analizar(TipoReenvio::Local, "0 localhost:22").is_err());
        assert!(Reenvio::analizar(TipoReenvio::Local, "hola adios").is_err());
    }

    #[test]
    fn analiza_las_comprobaciones_de_salud() {
        assert_eq!(Salud::analizar("").unwrap(), Salud::Escucha);
        assert_eq!(Salud::analizar("escucha").unwrap(), Salud::Escucha);
        assert_eq!(Salud::analizar("banner").unwrap(), Salud::Banner);
        assert_eq!(Salud::analizar("tcp").unwrap(), Salud::Conecta);
        assert_eq!(
            Salud::analizar("http:/api/health").unwrap(),
            Salud::Http {
                ruta: "/api/health".into()
            }
        );
        assert_eq!(
            Salud::analizar("http:/-/healthy").unwrap(),
            Salud::Http {
                ruta: "/-/healthy".into()
            }
        );
    }

    #[test]
    fn rechaza_saludes_mal_escritas() {
        // Sin la barra inicial la petición saldría malformada y el diagnóstico
        // sería peor que el problema.
        assert!(Salud::analizar("http:api/health").is_err());
        assert!(Salud::analizar("ping").is_err());
    }

    #[test]
    fn la_especificacion_va_y_vuelve() {
        for salud in [
            Salud::Escucha,
            Salud::Conecta,
            Salud::Banner,
            Salud::Http {
                ruta: "/-/healthy".into(),
            },
        ] {
            let texto = salud.especificacion();
            assert_eq!(
                Salud::analizar(&texto).unwrap(),
                salud,
                "falló con «{texto}»"
            );
        }
    }

    #[test]
    fn solo_las_saludes_profundas_atraviesan_el_tunel() {
        // La distinción decide la cadencia: la barata va en cada latido, la
        // que abre una conexión real no.
        assert!(!Salud::Escucha.atraviesa_el_tunel());
        assert!(Salud::Conecta.atraviesa_el_tunel());
        assert!(Salud::Banner.atraviesa_el_tunel());
        assert!(Salud::Http { ruta: "/".into() }.atraviesa_el_tunel());
    }

    #[test]
    fn el_id_del_tunel_es_estable() {
        let t = Tunel {
            alias: "preprod".into(),
            reenvio: Reenvio::local(
                Extremo::solo_puerto(3000),
                Extremo::nuevo("localhost", 3000),
            ),
        };
        assert_eq!(t.id(), "preprod|-L|3000");
    }

    #[test]
    fn detecta_claves_de_hardware() {
        let mut host = Host::nuevo("bastion");
        host.identidades.push("~/.ssh/id_ed25519_sk".into());
        assert!(host.usa_clave_hardware());

        let mut normal = Host::nuevo("bastion");
        normal.identidades.push("~/.ssh/id_ed25519".into());
        assert!(!normal.usa_clave_hardware());
    }

    #[test]
    fn rechaza_alias_con_comodin() {
        let host = Host::nuevo("tunel-*");
        assert!(host.validar().is_err());
    }

    #[test]
    fn rechaza_dos_reenvios_en_el_mismo_puerto() {
        let mut host = Host::nuevo("preprod");
        host.hostname = Some("interno".into());
        host.reenvios.push(Reenvio::local(
            Extremo::solo_puerto(3000),
            Extremo::nuevo("localhost", 3000),
        ));
        host.reenvios.push(Reenvio::local(
            Extremo::solo_puerto(3000),
            Extremo::nuevo("otro", 3000),
        ));
        assert!(host.validar().is_err());
    }

    #[test]
    fn rechaza_saltos_de_linea_en_cualquier_campo() {
        // Un «\n» dentro de un valor abriría directivas nuevas al escribir
        // el fichero: la nota es el campo más fácil de rellenar pegando.
        let mut host = Host::nuevo("preprod");
        host.hostname = Some("interno".into());
        host.nota = Some("línea\n    ProxyCommand touch /tmp/pwned".into());
        assert!(host.validar().is_err());

        let mut host = Host::nuevo("preprod");
        host.hostname = Some("interno\nProxyCommand true".into());
        assert!(host.validar().is_err());

        let mut host = Host::nuevo("preprod");
        host.hostname = Some("interno".into());
        host.reenvios.push(
            Reenvio::local(
                Extremo::solo_puerto(3000),
                Extremo::nuevo("localhost", 3000),
            )
            .con_salud(Salud::Http {
                ruta: "/salud\nProxyCommand true".into(),
            }),
        );
        assert!(host.validar().is_err());
    }

    #[test]
    fn rechaza_valores_que_ssh_tomaria_por_opciones() {
        // «ssh» no admite el separador «--»: un destino que empiece por «-»
        // se consumiría como opción aunque viaje como argumento separado.
        let host = Host::nuevo("-oProxyCommand=true");
        assert!(host.validar().is_err());

        let mut host = Host::nuevo("preprod");
        host.hostname = Some("-oProxyCommand=true".into());
        assert!(host.validar().is_err());

        let mut host = Host::nuevo("preprod");
        host.hostname = Some("interno".into());
        host.saltos.push("-J-abusivo".into());
        assert!(host.validar().is_err());
    }

    #[test]
    fn una_nota_con_acentos_y_espacios_sigue_siendo_valida() {
        let mut host = Host::nuevo("preprod");
        host.hostname = Some("interno".into());
        host.nota = Some("túnel de la base de datos — pedirlo a sistemas".into());
        host.identidades
            .push("/ruta/con espacios/id_ed25519".into());
        assert!(host.validar().is_ok());
    }
}
