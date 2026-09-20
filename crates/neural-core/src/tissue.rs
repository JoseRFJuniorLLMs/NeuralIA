//! Tecido neuronal: a matematica por tras do fundo animado da NeuralIA.
//!
//! E so aritmetica -- sem Win32, sem GDI, sem estado. Uma funcao do tempo para
//! uma lista de neuronios, ligacoes e impulsos, que quem desenha traduz para o
//! que tiver a mao. Vive aqui, e nao no binario Windows, por duas razoes: o
//! instalador precisa exatamente do mesmo fundo do navegador, e assim a unica
//! parte que se pode enganar sozinha -- as contas -- e testavel sem ecra.

/// Um neuronio. `energy` anda entre 0 e 1 e diz o quanto esta aceso agora.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    pub x: f64,
    pub y: f64,
    pub energy: f64,
}

/// Uma ligacao entre dois neuronios proximos. `closeness` anda entre 0 e 1:
/// 1 e encostado, 0 e no limite do alcance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    pub ax: f64,
    pub ay: f64,
    pub bx: f64,
    pub by: f64,
    pub closeness: f64,
}

/// Um impulso a viajar numa ligacao. `glow` acende a sair e apaga a chegar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pulse {
    pub x: f64,
    pub y: f64,
    pub glow: f64,
}

/// Uma descarga: dois neuronios encontraram-se e rebentaram um no outro.
/// `glow` vai de 0, no instante do contacto, a 1 quando estao em cima um do
/// outro; `radius` e o tamanho do clarao nesse momento.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Burst {
    pub x: f64,
    pub y: f64,
    pub radius: f64,
    pub glow: f64,
}

/// O que ha para desenhar num instante.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tissue {
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
    pub pulses: Vec<Pulse>,
    pub bursts: Vec<Burst>,
}

/// A tela onde o tecido cresce.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub width: f64,
    pub height: f64,
    /// Fator de DPI. Tudo o que e distancia multiplica por isto.
    pub scale: f64,
    /// Elipse de silencio: `(centro_x, centro_y, raio_x, raio_y)`. Nenhum
    /// neuronio entra aqui -- e onde vive a marca, e texto por cima de linhas
    /// nao se le.
    pub quiet: Option<(f64, f64, f64, f64)>,
}

impl Field {
    pub fn new(width: f64, height: f64, scale: f64) -> Self {
        Self {
            width,
            height,
            scale,
            quiet: None,
        }
    }

    pub fn with_quiet_ellipse(mut self, cx: f64, cy: f64, rx: f64, ry: f64) -> Self {
        self.quiet = Some((cx, cy, rx.max(1.0), ry.max(1.0)));
        self
    }
}

/// Ruido deterministico entre 0 e 1. Nao ha gerador aleatorio nenhum, porque o
/// quadro tem de sair igual para o mesmo instante -- senao o fundo treme
/// sempre que a janela se redesenha.
fn hash(mut value: u32) -> f64 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^= value >> 16;
    value as f64 / u32::MAX as f64
}

/// Quantos neuronios cabem nesta largura. Poucos parecem pobres, muitos viram
/// um borrao cinzento.
fn node_count(field: &Field) -> usize {
    ((field.width / (34.0 * field.scale.max(1.0))).round() as usize).clamp(28, 52)
}

/// Os neuronios no instante `seconds`.
pub fn nodes_at(field: &Field, seconds: f64) -> Vec<Node> {
    let scale = field.scale.max(1.0);
    let count = node_count(field);

    // Grelha com ruido: uma distribuicao puramente aleatoria faz grumos e
    // buracos, e nenhum dos dois se parece com tecido neuronal.
    let columns = (count as f64).sqrt().ceil().max(1.0) as usize;
    let rows = count.div_ceil(columns).max(1);
    let cell_w = field.width / columns as f64;
    let cell_h = field.height / rows as f64;

    let mut nodes = Vec::with_capacity(count);
    for i in 0..count {
        let seed = i as u32 + 1;
        let (col, row) = (i % columns, i / columns);
        let jitter_x = hash(seed.wrapping_mul(0x9e37_79b9)) - 0.5;
        let jitter_y = hash(seed.wrapping_mul(0x85eb_ca6b)) - 0.5;

        let mut x = (col as f64 + 0.5) * cell_w + jitter_x * cell_w * 0.7;
        let mut y = (row as f64 + 0.5) * cell_h + jitter_y * cell_h * 0.7;

        // Orbitas largas e rapidas. O tecido nao "respira" -- trabalha: cada
        // neuronio percorre boa parte da sua celula, e por isso encontra os
        // vizinhos em vez de ficar a acenar no mesmo sitio. E esse encontro
        // que produz as descargas.
        let phase = hash(seed.wrapping_mul(0xc2b2_ae35)) * std::f64::consts::TAU;
        let speed = 0.55 + hash(seed.wrapping_mul(0x27d4_eb2d)) * 0.75;
        let swing_x = cell_w * 0.40;
        let swing_y = cell_h * 0.40;
        x += (seconds * speed + phase).sin() * swing_x;
        y += (seconds * speed * 0.83 + phase * 1.7).cos() * swing_y;
        let _ = scale;

        if let Some((cx, cy, rx, ry)) = field.quiet {
            // Empurrar para fora da elipse pela normal, mantendo a direcao.
            let nx = (x - cx) / rx;
            let ny = (y - cy) / ry;
            let radial = (nx * nx + ny * ny).sqrt();
            if radial <= f64::EPSILON {
                x = cx + rx;
            } else if radial < 1.0 {
                let push = 1.0 / radial;
                x = cx + (x - cx) * push;
                y = cy + (y - cy) * push;
            }
        }

        // Cada neuronio pulsa no seu proprio ritmo.
        let beat = hash(seed.wrapping_mul(0x1656_67b1)) * std::f64::consts::TAU;
        let energy = 0.45 + 0.55 * (seconds * 0.9 + beat).sin().abs();
        nodes.push(Node { x, y, energy });
    }
    nodes
}

/// O alcance de uma sinapse. Mais do que isto, os neuronios ignoram-se.
///
/// Sai do espacamento medio, nao de um numero fixo: com um alcance fixo, uma
/// janela larga afasta as colunas para alem dele e o que se ve sao fios
/// verticais de contas soltas -- exatamente o que nao e uma rede. Amarrado ao
/// espacamento, a vizinhanca de cada neuronio e a mesma em qualquer tamanho.
pub fn max_link(field: &Field) -> f64 {
    let spacing = (field.width.max(1.0) * field.height.max(1.0) / node_count(field) as f64).sqrt();
    (spacing * 1.35).max(90.0 * field.scale.max(1.0))
}

/// O tecido inteiro num instante: neuronios, ligacoes e impulsos.
/// A que distancia dois neuronios se consideram em contacto. Mais do que isto
/// e so vizinhanca; menos do que isto e descarga.
pub fn contact_distance(field: &Field) -> f64 {
    max_link(field) * 0.13
}

pub fn tissue_at(field: &Field, seconds: f64) -> Tissue {
    let nodes = nodes_at(field, seconds);
    let reach = max_link(field);
    let contact = contact_distance(field);
    let mut links = Vec::new();
    let mut pulses = Vec::new();
    let mut bursts = Vec::new();

    for i in 0..nodes.len() {
        for j in (i + 1)..nodes.len() {
            let (a, b) = (nodes[i], nodes[j]);
            let distance = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
            if distance > reach {
                continue;
            }
            // Encontraram-se: rebenta no ponto de contacto. A forca cresce a
            // medida que se aproximam, portanto o clarao acende e apaga
            // sozinho com o movimento -- nao ha estado nenhum a guardar.
            if distance < contact {
                let force = 1.0 - distance / contact;
                bursts.push(Burst {
                    x: (a.x + b.x) / 2.0,
                    y: (a.y + b.y) / 2.0,
                    radius: contact * (0.45 + 1.25 * force),
                    glow: force,
                });
            }
            let closeness = 1.0 - distance / reach;
            links.push(Link {
                ax: a.x,
                ay: a.y,
                bx: b.x,
                by: b.y,
                closeness,
            });

            // So as ligacoes curtas transmitem, e so algumas: uma sinapse a
            // disparar em cada ligacao ao mesmo tempo seria ruido outra vez.
            let seed = (i as u32 + 1).wrapping_mul(0x85eb_ca6b) ^ (j as u32 + 1);
            if closeness < 0.45 || hash(seed) > 0.38 {
                continue;
            }
            let speed = 0.55 + hash(seed.wrapping_mul(0xc2b2_ae35)) * 0.70;
            let offset = hash(seed.wrapping_mul(0xd3a2_646c));
            let travel = (seconds * speed + offset).fract();
            pulses.push(Pulse {
                x: a.x + (b.x - a.x) * travel,
                y: a.y + (b.y - a.y) * travel,
                glow: (travel * std::f64::consts::PI).sin(),
            });
        }
    }

    Tissue {
        nodes,
        links,
        pulses,
        bursts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field() -> Field {
        Field::new(960.0, 540.0, 1.0)
    }

    #[test]
    fn the_same_instant_always_draws_the_same_frame() {
        // Sem isto o fundo treme: cada WM_PAINT desenhava outra coisa para o
        // mesmo relogio.
        let first = tissue_at(&field(), 12.5);
        let second = tissue_at(&field(), 12.5);
        assert_eq!(first, second);
        assert_ne!(first, tissue_at(&field(), 12.6));
    }

    #[test]
    fn the_tissue_is_dense_enough_to_read_as_a_network() {
        let tissue = tissue_at(&field(), 3.0);
        assert!(
            (28..=52).contains(&tissue.nodes.len()),
            "neuronios: {}",
            tissue.nodes.len()
        );
        assert!(
            tissue.links.len() > tissue.nodes.len(),
            "com menos ligacoes do que neuronios isto sao pontos soltos, nao \
             uma rede: {} ligacoes para {} neuronios",
            tissue.links.len(),
            tissue.nodes.len()
        );
    }

    #[test]
    fn nothing_is_drawn_inside_the_quiet_ellipse() {
        // A marca fica aqui. Linhas por baixo de texto tornam-no ilegivel.
        let field = field().with_quiet_ellipse(480.0, 270.0, 220.0, 90.0);
        for step in 0..40 {
            let seconds = step as f64 * 0.37;
            for node in nodes_at(&field, seconds) {
                let nx = (node.x - 480.0) / 220.0;
                let ny = (node.y - 270.0) / 90.0;
                let radial = (nx * nx + ny * ny).sqrt();
                assert!(
                    radial >= 0.999,
                    "neuronio dentro da zona de silencio em t={seconds}: ({}, {}) radial {radial}",
                    node.x,
                    node.y
                );
            }
        }
    }

    #[test]
    fn links_only_join_neighbours_and_pulses_ride_them() {
        let field = field();
        let reach = max_link(&field);
        let tissue = tissue_at(&field, 7.25);
        for link in &tissue.links {
            let distance = ((link.ax - link.bx).powi(2) + (link.ay - link.by).powi(2)).sqrt();
            assert!(distance <= reach + 1e-9, "ligacao longa demais: {distance}");
            assert!((0.0..=1.0).contains(&link.closeness));
        }
        assert!(!tissue.pulses.is_empty(), "uma rede parada nao e sinapse");
        assert!(
            tissue.pulses.len() < tissue.links.len(),
            "todas as ligacoes a disparar ao mesmo tempo e ruido, nao sinal"
        );
        for pulse in &tissue.pulses {
            assert!(
                (0.0..=1.0).contains(&pulse.glow),
                "brilho fora de escala: {}",
                pulse.glow
            );
        }
    }

    #[test]
    fn energy_stays_within_the_range_the_painter_assumes() {
        for step in 0..60 {
            for node in nodes_at(&field(), step as f64 * 0.21) {
                assert!(
                    (0.45..=1.0).contains(&node.energy),
                    "energia fora de escala: {}",
                    node.energy
                );
            }
        }
    }

    #[test]
    fn every_window_size_stays_a_web_and_never_becomes_beads_on_strings() {
        // Com alcance fixo, uma janela larga afasta as colunas para alem dele:
        // sobram fios verticais de contas soltas. Este e o teste que o dono
        // teria escrito quando disse que os neuronios nao eram sinapses.
        for (width, height) in [
            (360.0, 220.0),
            (720.0, 460.0),
            (960.0, 540.0),
            (1440.0, 900.0),
            (1920.0, 1080.0),
            (2560.0, 1440.0),
        ] {
            let field = Field::new(width, height, 1.0);
            let tissue = tissue_at(&field, 5.0);
            let per_node = tissue.links.len() as f64 / tissue.nodes.len() as f64;
            assert!(
                per_node >= 1.5,
                "{width}x{height}: {:.2} ligacoes por neuronio -- isto sao \
                 contas num fio, nao uma rede",
                per_node
            );
            assert!(
                per_node <= 6.0,
                "{width}x{height}: {:.2} ligacoes por neuronio -- isto e uma \
                 mancha, nao uma rede",
                per_node
            );
        }
    }

    #[test]
    fn neurons_actually_travel_instead_of_waving_in_place() {
        // Antes cada neuronio oscilava ~9 px e nunca chegava ao vizinho. Sem
        // deslocacao nao ha encontro, e sem encontro nao ha descarga.
        let field = field();
        let start = nodes_at(&field, 0.0);
        let mut furthest = 0.0f64;
        for step in 1..120 {
            let later = nodes_at(&field, step as f64 * 0.05);
            for (a, b) in start.iter().zip(later.iter()) {
                furthest = furthest.max(((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt());
            }
        }
        let spacing = (field.width * field.height / start.len() as f64).sqrt();
        assert!(
            furthest > spacing * 0.5,
            "em 6 segundos o neuronio que mais andou fez {furthest:.0} px, e o \
             espacamento e {spacing:.0} px: isto nao e um cerebro, e um mobile"
        );
    }

    #[test]
    fn neurons_that_meet_discharge_and_the_flash_dies_with_the_contact() {
        // O pedido era este: tem de rebentar quando se encontram.
        let field = field();
        let contact = contact_distance(&field);
        let mut seen = 0usize;
        for step in 0..600 {
            let seconds = step as f64 * 0.05;
            let tissue = tissue_at(&field, seconds);
            seen += tissue.bursts.len();
            for burst in &tissue.bursts {
                assert!(
                    (0.0..=1.0).contains(&burst.glow),
                    "clarao fora de escala: {}",
                    burst.glow
                );
                assert!(burst.radius > 0.0);
                // Um clarao so pode existir onde ha mesmo dois neuronios
                // encostados -- senao sao faiscas do nada.
                let touching = tissue.nodes.iter().any(|a| {
                    tissue.nodes.iter().any(|b| {
                        let d = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
                        d > 0.0
                            && d < contact
                            && ((a.x + b.x) / 2.0 - burst.x).abs() < 1e-6
                            && ((a.y + b.y) / 2.0 - burst.y).abs() < 1e-6
                    })
                });
                assert!(
                    touching,
                    "clarao em ({}, {}) sem contacto",
                    burst.x, burst.y
                );
            }
        }
        assert!(
            seen > 40,
            "em 30 segundos houve {seen} descargas: o cerebro esta desligado"
        );
    }

    #[test]
    fn the_flash_is_brightest_when_they_are_on_top_of_each_other() {
        // Um clarao de intensidade fixa le-se como um ponto aceso, nao como
        // uma descarga: tem de crescer com a aproximacao.
        let field = field();
        let contact = contact_distance(&field);
        let mut checked = 0usize;
        let mut brightest = 0.0f64;
        for step in 0..600 {
            let tissue = tissue_at(&field, step as f64 * 0.05);
            for burst in &tissue.bursts {
                // Encontrar o par que produziu este clarao e confirmar que o
                // brilho e mesmo a aproximacao deles, e nao um valor fixo.
                let mut matched = false;
                for i in 0..tissue.nodes.len() {
                    for j in (i + 1)..tissue.nodes.len() {
                        let (a, b) = (tissue.nodes[i], tissue.nodes[j]);
                        if ((a.x + b.x) / 2.0 - burst.x).abs() > 1e-9
                            || ((a.y + b.y) / 2.0 - burst.y).abs() > 1e-9
                        {
                            continue;
                        }
                        let d = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
                        assert!(
                            (burst.glow - (1.0 - d / contact)).abs() < 1e-9,
                            "a {d:.1} px o clarao e {:.3}, nao a aproximacao",
                            burst.glow
                        );
                        matched = true;
                    }
                }
                assert!(matched, "clarao sem par que o explique");
                brightest = brightest.max(burst.glow);
                checked += 1;
            }
        }
        assert!(checked > 40, "so {checked} descargas para medir");
        assert!(
            brightest > 0.5,
            "o clarao mais forte em 30 segundos foi {brightest:.2}: eles \
             aproximam-se mas nunca se encontram a serio"
        );
    }

    #[test]
    fn a_narrow_window_still_grows_tissue() {
        // A janela do instalador e estreita. Sem o minimo, ficava um punhado
        // de pontos soltos.
        let narrow = Field::new(360.0, 220.0, 1.0);
        let tissue = tissue_at(&narrow, 1.0);
        assert_eq!(tissue.nodes.len(), 28);
        assert!(!tissue.links.is_empty());
    }
}
