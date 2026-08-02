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

/// Un reenvío concreto.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Reenvio {
    pub tipo: TipoReenvio,
    pub escucha: Extremo,
    /// `None` solo para `DynamicForward`.
    pub destino: Option<Extremo>,
    /// Cómo comprobar que está vivo de verdad.
    pub salud: Salud,
}

impl Reenvio {
    pub fn local(escucha: Extremo, destino: Extremo) -> Self {
        Reenvio {
            tipo: TipoReenvio::Local,
            escucha,
            destino: Some(destino),
            salud: Salud::default(),
        }
    }

    pub fn dinamico(escucha: Extremo) -> Self {
        Reenvio {
            tipo: TipoReenvio::Dinamico,
            escucha,
            destino: None,
            salud: Salud::default(),
        }
    }

    /// El mismo reenvío con otra comprobación de salud.
    pub fn con_salud(mut self, salud: Salud) -> Self {
        self.salud = salud;
        self
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
            }),
            (TipoReenvio::Local | TipoReenvio::Remoto, [escucha, destino]) => Ok(Reenvio {
                tipo,
                escucha: Extremo::analizar(escucha)?,
                destino: Some(Extremo::analizar(destino)?),
                salud: Salud::default(),
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
                    }),
                    [enlace, escucha, host, puerto] => Ok(Reenvio {
                        tipo,
                        escucha: Extremo::nuevo(*enlace, analizar_puerto(escucha)?),
                        destino: Some(Extremo::nuevo(*host, analizar_puerto(puerto)?)),
                        salud: Salud::default(),
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

    /// Comprueba que la definición es coherente antes de guardarla.
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
}
