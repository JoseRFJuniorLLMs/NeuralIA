//! Tecido neuronal: a matematica por tras do fundo animado da NeuralIA.
//!
//! E so aritmetica -- sem Win32, sem GDI, sem estado. Uma funcao do tempo para
//! uma lista de neuronios, ligacoes e impulsos, que quem desenha traduz para o
//! que tiver a mao. Vive aqui, e nao no binario Windows, por duas razoes: o
//! instalador precisa exatamente do mesmo fundo do navegador, e assim a unica
//! parte que se pode enganar sozinha -- as contas -- e testavel sem ecra.

/// O corpo de um neuronio. `energy` anda entre 0 e 1 e diz o quanto esta aceso
/// agora; `depth` entre 0 e 1 diz o quao a frente esta -- e o que da camadas ao
/// tecido em vez de uma folha plana de pontos iguais.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    pub x: f64,
    pub y: f64,
    pub energy: f64,
    pub depth: f64,
}

/// Um segmento de ramagem. `weight` entre 0 e 1 diz o quao grosso e aceso:
/// 1 no tronco que sai do corpo, cada vez menos a medida que bifurca.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Branch {
    pub ax: f64,
    pub ay: f64,
    pub bx: f64,
    pub by: f64,
    pub weight: f64,
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
    /// A ramagem, do tronco aos fios mais finos. Desenha-se primeiro: e o que
    /// esta por tras de tudo.
    pub branches: Vec<Branch>,
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

/// Quantos somas cabem nesta tela.
///
/// Sai da **area**, nao da largura: uma janela larga e baixa tinha a mesma
/// contagem de uma alta, e ficava rala. E divide-se pelo DPI, senao o mesmo
/// ecra a 200% levava quatro vezes mais neuronios do que a 100% e virava uma
/// mancha.
fn node_count(field: &Field) -> usize {
    let scale = field.scale.max(1.0);
    let area = field.width.max(1.0) * field.height.max(1.0) / (scale * scale);
    ((area / 7200.0).round() as usize).clamp(70, 150)
}

/// O espacamento medio entre somas. E a unidade de tudo o resto: o alcance das
/// sinapses, o comprimento dos dendritos e o raio de contacto saem daqui, para
/// o tecido ter o mesmo aspeto em qualquer tamanho de janela.
pub fn spacing(field: &Field) -> f64 {
    (field.width.max(1.0) * field.height.max(1.0) / node_count(field) as f64).sqrt()
}

/// Os somas no instante `seconds`.
pub fn nodes_at(field: &Field, seconds: f64) -> Vec<Node> {
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
        x += (seconds * speed + phase).sin() * cell_w * 0.40;
        y += (seconds * speed * 0.83 + phase * 1.7).cos() * cell_h * 0.40;

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

        // Profundidade: uns estao a frente, outros ao fundo. Sem isto o tecido
        // e uma folha plana de pontos todos iguais; com isto ganha camadas,
        // que e o que faz parecer volume.
        let depth = 0.25 + hash(seed.wrapping_mul(0x2545_f491)) * 0.75;
        // Cada neuronio pulsa no seu proprio ritmo.
        let beat = hash(seed.wrapping_mul(0x1656_67b1)) * std::f64::consts::TAU;
        let energy = 0.45 + 0.55 * (seconds * 0.9 + beat).sin().abs();
        nodes.push(Node {
            x,
            y,
            energy,
            depth,
        });
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
    (spacing(field) * 1.55).max(90.0 * field.scale.max(1.0))
}

/// Se um ponto cai dentro da elipse de silencio. Sem elipse, nada cai dentro.
fn inside_quiet(field: &Field, x: f64, y: f64) -> bool {
    let Some((cx, cy, rx, ry)) = field.quiet else {
        return false;
    };
    let nx = (x - cx) / rx;
    let ny = (y - cy) / ry;
    nx * nx + ny * ny < 1.0
}

/// A que distancia dois neuronios se consideram em contacto. Mais do que isto
/// e so vizinhanca; menos do que isto e descarga.
pub fn contact_distance(field: &Field) -> f64 {
    max_link(field) * 0.13
}

/// Os dendritos de um soma: arvores curtas que saem dele em todas as direcoes.
///
/// E o que distingue tecido neuronal de um grafo de pontos e linhas. Cada
/// braco sai do corpo, bifurca uma vez e bifurca outra, afinando e apagando a
/// cada nivel -- como nas fotografias de microscopia, onde o que se ve nao sao
/// ligacoes mas ramagem.
fn dendrites_of(field: &Field, node: &Node, seed: u32, seconds: f64, out: &mut Vec<Branch>) {
    let reach = spacing(field) * 0.78 * (0.55 + node.depth * 0.65);
    let arms = 4 + (hash(seed.wrapping_mul(0x7feb_352d)) * 3.0) as usize;
    let base = hash(seed.wrapping_mul(0x165e_1b7f)) * std::f64::consts::TAU;
    // Rotacao lenta, e em sentidos diferentes: a ramagem mexe-se sem que se
    // perceba um padrao a repetir.
    let spin = (hash(seed.wrapping_mul(0x9e37_79b1)) - 0.5) * 0.18;

    let mut push = |ax: f64, ay: f64, bx: f64, by: f64, weight: f64| {
        // A ramagem respeita a zona da marca como tudo o resto.
        if inside_quiet(field, ax, ay) || inside_quiet(field, bx, by) {
            return;
        }
        out.push(Branch {
            ax,
            ay,
            bx,
            by,
            weight,
        });
    };

    for arm in 0..arms {
        let arm_seed = seed.wrapping_mul(0x27d4_eb2d) ^ (arm as u32 + 1);
        let wobble = (seconds * 0.7 + hash(arm_seed) * std::f64::consts::TAU).sin() * 0.14;
        let angle =
            base + seconds * spin + arm as f64 * std::f64::consts::TAU / arms as f64 + wobble;
        let length = reach * (0.7 + hash(arm_seed.wrapping_mul(0x85eb_ca6b)) * 0.6);

        let tip_x = node.x + angle.cos() * length;
        let tip_y = node.y + angle.sin() * length;
        push(node.x, node.y, tip_x, tip_y, 1.0);

        // Primeira bifurcacao.
        for (fork, side) in [(0u32, -1.0f64), (1, 1.0)] {
            let fork_seed = arm_seed.wrapping_mul(0x846c_a68b) ^ (fork + 1);
            let spread = 0.36 + hash(fork_seed) * 0.34;
            let branch_angle = angle + side * spread;
            let branch_len = length * (0.48 + hash(fork_seed.wrapping_mul(0xc2b2_ae35)) * 0.26);
            let bx = tip_x + branch_angle.cos() * branch_len;
            let by = tip_y + branch_angle.sin() * branch_len;
            push(tip_x, tip_y, bx, by, 0.6);

            // Segunda bifurcacao: os fios mais finos, quase apagados.
            for (twig, twig_side) in [(0u32, -1.0f64), (1, 1.0)] {
                let twig_seed = fork_seed.wrapping_mul(0xd3a2_646c) ^ (twig + 1);
                let twig_angle = branch_angle + twig_side * (0.30 + hash(twig_seed) * 0.30);
                let twig_len = branch_len * 0.55;
                push(
                    bx,
                    by,
                    bx + twig_angle.cos() * twig_len,
                    by + twig_angle.sin() * twig_len,
                    0.32,
                );
            }
        }
    }
}

/// O tecido inteiro num instante: somas, ramagem, sinapses, impulsos e
/// descargas.
pub fn tissue_at(field: &Field, seconds: f64) -> Tissue {
    let nodes = nodes_at(field, seconds);
    let reach = max_link(field);
    let contact = contact_distance(field);
    let mut branches = Vec::with_capacity(nodes.len() * 28);
    let mut links = Vec::new();
    let mut pulses = Vec::new();
    let mut bursts = Vec::new();

    for (index, node) in nodes.iter().enumerate() {
        dendrites_of(field, node, index as u32 + 1, seconds, &mut branches);
    }

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
                let (mx, my) = ((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
                // Os neuronios ficam fora da zona de silencio, mas o ponto
                // medio de dois que a ladeiam cai la dentro, e uma faisca em
                // cima da marca chama a atencao exatamente para o sitio que
                // tem de ficar limpo.
                if !inside_quiet(field, mx, my) {
                    let force = 1.0 - distance / contact;
                    bursts.push(Burst {
                        x: mx,
                        y: my,
                        radius: contact * (0.45 + 1.25 * force),
                        glow: force,
                    });
                }
            }
            let closeness = 1.0 - distance / reach;
            links.push(Link {
                ax: a.x,
                ay: a.y,
                bx: b.x,
                by: b.y,
                closeness,
            });

            // Nem todas as ligacoes transmitem ao mesmo tempo -- isso seria
            // ruido -- mas transmitem muitas, e depressa: e o transito que faz
            // o tecido parecer vivo em vez de desenhado.
            let seed = (i as u32 + 1).wrapping_mul(0x85eb_ca6b) ^ (j as u32 + 1);
            if closeness < 0.25 || hash(seed) > 0.55 {
                continue;
            }
            let speed = 1.10 + hash(seed.wrapping_mul(0xc2b2_ae35)) * 1.30;
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
        branches,
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
            (70..=150).contains(&tissue.nodes.len()),
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
                per_node >= 2.5,
                "{width}x{height}: {:.2} ligacoes por neuronio -- isto sao \
                 contas num fio, nao tecido",
                per_node
            );
            assert!(
                per_node <= 12.0,
                "{width}x{height}: {:.2} ligacoes por neuronio -- a esta \
                 densidade deixa de haver buracos e fica uma mancha",
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
    fn no_discharge_lights_up_inside_the_quiet_ellipse() {
        // Os neuronios ja ficam de fora, mas o ponto medio de dois que ladeiam
        // a zona cai la dentro. Uma faisca ali chama a atencao exatamente para
        // o sitio que tinha de ficar limpo.
        let field = field().with_quiet_ellipse(480.0, 270.0, 220.0, 90.0);
        let mut seen = 0usize;
        for step in 0..600 {
            for burst in tissue_at(&field, step as f64 * 0.05).bursts {
                let nx = (burst.x - 480.0) / 220.0;
                let ny = (burst.y - 270.0) / 90.0;
                assert!(
                    nx * nx + ny * ny >= 1.0,
                    "descarga dentro da zona de silencio em ({}, {})",
                    burst.x,
                    burst.y
                );
                seen += 1;
            }
        }
        assert!(seen > 20, "so {seen} descargas: nada para verificar");
    }

    #[test]
    fn every_soma_grows_a_branching_tree_and_not_a_star() {
        // O que distingue tecido de um grafo de pontos e linhas e a ramagem:
        // cada braco tem de bifurcar, e os fios finos tem de ser mais finos.
        let field = field();
        let tissue = tissue_at(&field, 4.0);
        assert!(
            tissue.branches.len() > tissue.nodes.len() * 12,
            "{} ramos para {} somas: isto e uma estrela, nao uma arvore",
            tissue.branches.len(),
            tissue.nodes.len()
        );

        let trunks = tissue.branches.iter().filter(|b| b.weight > 0.9).count();
        let twigs = tissue.branches.iter().filter(|b| b.weight < 0.4).count();
        assert!(
            twigs > trunks * 2,
            "{twigs} fios finos para {trunks} troncos: a ramagem nao esta a \
             bifurcar duas vezes"
        );
        for branch in &tissue.branches {
            assert!((0.0..=1.0).contains(&branch.weight));
        }
    }

    #[test]
    fn the_branches_stay_local_instead_of_crossing_the_screen() {
        // Um dendrito que atravessa a tela deixa de se ler como ramagem e
        // passa a parecer uma ligacao errada.
        let field = field();
        let limit = spacing(&field) * 1.6;
        for step in 0..30 {
            for branch in tissue_at(&field, step as f64 * 0.4).branches {
                let length =
                    ((branch.ax - branch.bx).powi(2) + (branch.ay - branch.by).powi(2)).sqrt();
                assert!(
                    length <= limit,
                    "ramo de {length:.0} px para um espacamento de {:.0} px",
                    spacing(&field)
                );
            }
        }
    }

    #[test]
    fn no_branch_reaches_into_the_quiet_ellipse() {
        // A ramagem e muito mais densa do que os somas: se nao respeitasse a
        // zona limpa, tapava a marca sozinha.
        let field = field().with_quiet_ellipse(480.0, 270.0, 220.0, 90.0);
        for step in 0..30 {
            for branch in tissue_at(&field, step as f64 * 0.4).branches {
                for (x, y) in [(branch.ax, branch.ay), (branch.bx, branch.by)] {
                    let nx = (x - 480.0) / 220.0;
                    let ny = (y - 270.0) / 90.0;
                    assert!(
                        nx * nx + ny * ny >= 1.0,
                        "ramo dentro da zona de silencio em ({x}, {y})"
                    );
                }
            }
        }
    }

    #[test]
    fn the_frame_stays_within_what_gdi_can_draw_thirty_times_a_second() {
        // Contagens, nao segundos: um limite em milissegundos falha sozinho
        // num CI lento. O que importa e nao pedir ao GDI mais linhas do que
        // ele desenha entre dois quadros.
        for (w, h, scale) in [
            (1920.0, 1080.0, 1.0),
            (2906.0, 1826.0, 2.0),
            (1520.0, 1000.0, 2.0),
        ] {
            let tissue = tissue_at(&Field::new(w, h, scale), 5.0);
            assert!(
                tissue.branches.len() <= 6000,
                "{w}x{h}@{scale}: {} ramos",
                tissue.branches.len()
            );
            assert!(
                tissue.links.len() <= 1400,
                "{w}x{h}@{scale}: {} ligacoes",
                tissue.links.len()
            );
        }
    }

    #[test]
    fn pulses_travel_fast_enough_to_read_as_traffic() {
        // "Esta lento" foi a queixa. Um impulso tem de atravessar a sua
        // ligacao em menos de um segundo, senao parece um ponto parado.
        let field = field();
        let mut moved = 0usize;
        let first = tissue_at(&field, 10.0);
        let second = tissue_at(&field, 10.25);
        for a in &first.pulses {
            if second
                .pulses
                .iter()
                .all(|b| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt() > 4.0)
            {
                moved += 1;
            }
        }
        assert!(
            moved * 2 > first.pulses.len(),
            "so {moved} de {} impulsos se mexeram em um quarto de segundo",
            first.pulses.len()
        );
    }

    #[test]
    fn a_narrow_window_still_grows_tissue() {
        // A janela do instalador e estreita. Sem o minimo, ficava um punhado
        // de pontos soltos.
        let narrow = Field::new(360.0, 220.0, 1.0);
        let tissue = tissue_at(&narrow, 1.0);
        assert_eq!(tissue.nodes.len(), 70);
        assert!(!tissue.links.is_empty());
        assert!(!tissue.branches.is_empty());
    }
}
