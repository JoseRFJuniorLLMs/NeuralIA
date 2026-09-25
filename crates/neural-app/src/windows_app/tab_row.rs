use super::*;

/// As cores que um grupo de abas pode ter: as nove do Chrome, com os nomes
/// dele. Poucas e nomeadas: uma paleta aberta obrigaria a um seletor, e o que
/// se quer e distinguir grupos de relance, nao escolher tons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum GroupColor {
    Blue,
    Green,
    Amber,
    Pink,
    Purple,
    Slate,
    Red,
    Cyan,
    Orange,
}

impl GroupColor {
    /// Pela ordem do seletor do Chrome.
    pub(in crate::windows_app) const ALL: [Self; 9] = [
        Self::Slate,
        Self::Blue,
        Self::Red,
        Self::Amber,
        Self::Green,
        Self::Pink,
        Self::Purple,
        Self::Cyan,
        Self::Orange,
    ];

    pub(in crate::windows_app) fn rgb(self) -> Rgb {
        match self {
            Self::Blue => (66, 133, 244),
            Self::Green => (52, 168, 83),
            Self::Amber => (244, 180, 0),
            Self::Pink => (233, 30, 99),
            Self::Purple => (156, 39, 176),
            Self::Slate => (96, 125, 139),
            Self::Red => (217, 48, 37),
            Self::Cyan => (0, 131, 143),
            Self::Orange => (250, 144, 62),
        }
    }

    /// A proxima cor por usar numa coluna, para dois grupos seguidos nao
    /// nascerem iguais. O azul primeiro, o cinza por ultimo.
    pub(in crate::windows_app) fn next(used: &[Self]) -> Self {
        [
            Self::Blue,
            Self::Red,
            Self::Amber,
            Self::Green,
            Self::Pink,
            Self::Purple,
            Self::Cyan,
            Self::Orange,
            Self::Slate,
        ]
        .into_iter()
        .find(|color| !used.contains(color))
        .unwrap_or(Self::Blue)
    }
}

/// Um grupo de abas na barra de titulo: nome, cor e se esta fechado.
#[derive(Debug, Clone)]
pub(in crate::windows_app) struct ContextGroup {
    pub(in crate::windows_app) id: u64,
    pub(in crate::windows_app) name: String,
    pub(in crate::windows_app) color: GroupColor,
    pub(in crate::windows_app) collapsed: bool,
}

/// Uma aba de contexto. O `group` e o id do grupo, nao um indice: fechar um
/// grupo no meio nao pode renumerar as abas dos outros.
#[derive(Debug, Clone)]
pub(in crate::windows_app) struct ContextTab {
    /// Identidade estavel. A URL pode repetir em grupos diferentes e por isso
    /// nunca serve para decidir qual aba esta aberta ou deve ser fechada.
    pub(in crate::windows_app) id: u64,
    pub(in crate::windows_app) url: String,
    pub(in crate::windows_app) group: Option<u64>,
}

/// Um lugar na fila de abas de uma coluna: ou a pilula de um grupo, ou uma aba.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum TabSlot {
    /// Indice do grupo dentro de `groups` daquela coluna.
    Group(usize),
    /// Indice da aba dentro de `contexts` daquela coluna.
    Tab(usize),
}

/// A fila visivel de uma coluna, ja cortada ao que cabe na barra.
#[derive(Debug, Clone, Copy)]
pub(in crate::windows_app) struct TabRow {
    pub(in crate::windows_app) slots: [TabSlot; MAX_VISIBLE_TAB_SLOTS],
    /// Para cada `TabSlot::Tab`, o indice do grupo a que a aba pertence. A
    /// barra precisa disto para saber ate onde vai o sublinhado do grupo.
    pub(in crate::windows_app) owners: [Option<usize>; MAX_VISIBLE_TAB_SLOTS],
    /// Para cada pilula, quantas abas do grupo nao estao na fila (recolhidas
    /// ou cortadas).
    pub(in crate::windows_app) behind: [usize; MAX_VISIBLE_TAB_SLOTS],
    /// Para cada pilula, se o grupo esta recolhido: as abas dele que nao
    /// estao na fila estao escondidas de proposito, nao cortadas.
    pub(in crate::windows_app) collapsed: [bool; MAX_VISIBLE_TAB_SLOTS],
    /// A aba aberta ao lado e a aba em que o dono acabou de mexer: a barra
    /// estreita corta tudo o resto antes delas.
    pub(in crate::windows_app) pinned: [bool; MAX_VISIBLE_TAB_SLOTS],
    /// Quantas abas a coluna tem ao todo.
    pub(in crate::windows_app) total_tabs: usize,
    pub(in crate::windows_app) len: usize,
}

impl TabRow {
    pub(in crate::windows_app) fn empty() -> Self {
        Self {
            slots: [TabSlot::Tab(0); MAX_VISIBLE_TAB_SLOTS],
            owners: [None; MAX_VISIBLE_TAB_SLOTS],
            behind: [0; MAX_VISIBLE_TAB_SLOTS],
            collapsed: [false; MAX_VISIBLE_TAB_SLOTS],
            pinned: [false; MAX_VISIBLE_TAB_SLOTS],
            total_tabs: 0,
            len: 0,
        }
    }

    /// Quantas abas da coluna ficam fora da vista se a barra desenhar so os
    /// lugares `drawn` da fila: nem desenhadas, nem atras de uma pilula
    /// recolhida desenhada. Sao as que o "‹N" conta.
    pub(in crate::windows_app) fn hidden_tabs(&self, drawn: &[bool]) -> usize {
        let mut seen = 0usize;
        for position in 0..self.len {
            if !drawn.get(position).copied().unwrap_or(false) {
                continue;
            }
            match self.slots[position] {
                TabSlot::Tab(_) => seen += 1,
                TabSlot::Group(_) if self.collapsed[position] => seen += self.behind[position],
                TabSlot::Group(_) => {}
            }
        }
        self.total_tabs.saturating_sub(seen)
    }

    pub(in crate::windows_app) fn push(&mut self, slot: TabSlot) {
        if self.len < MAX_VISIBLE_TAB_SLOTS {
            self.slots[self.len] = slot;
            self.len += 1;
        }
    }

    pub(in crate::windows_app) fn visible(&self) -> &[TabSlot] {
        &self.slots[..self.len]
    }

    /// O grupo da aba no lugar `position` da fila (`None` para pilulas e
    /// abas soltas).
    pub(in crate::windows_app) fn owner(&self, position: usize) -> Option<usize> {
        self.owners.get(position).copied().flatten()
    }

    /// Uma coluna sem grupo nenhum: as ultimas abas, como era antes de existirem
    /// grupos. Serve os chamadores que so sabem contar abas.
    #[cfg(test)]
    pub(in crate::windows_app) fn plain(count: usize) -> Self {
        let mut row = Self::empty();
        let shown = count.min(MAX_VISIBLE_CONTEXT_TABS);
        for offset in 0..shown {
            row.push(TabSlot::Tab(count - shown + offset));
        }
        row.total_tabs = count;
        row
    }
}

/// Atalho dos testes antigos: nenhuma aba aberta ao lado nesta coluna.
#[cfg(test)]
pub(in crate::windows_app) fn plan_tab_row(tabs: &[ContextTab], groups: &[ContextGroup]) -> TabRow {
    plan_tab_row_with_active(tabs, groups, None)
}

/// A fila de uma coluna sem aba nenhuma acabada de mexer.
#[cfg(test)]
pub(in crate::windows_app) fn plan_tab_row_with_active(
    tabs: &[ContextTab],
    groups: &[ContextGroup],
    active: Option<u64>,
) -> TabRow {
    plan_tab_row_focused(tabs, groups, active, None)
}

/// Decide o que aparece na barra de uma coluna: a pilula de cada grupo antes da
/// sua primeira aba, as abas de um grupo recolhido escondidas, e no maximo
/// `MAX_VISIBLE_CONTEXT_TABS` abas seguidas -- as mais recentes, ou as que
/// rodeiam as ancoras.
///
/// Invariantes que os testes prendem:
/// - uma aba cujo grupo ja nao existe volta a ser solta em vez de desaparecer;
/// - nunca sobra uma aba agrupada sem a pilula do seu grupo antes dela: se o
///   corte cai a meio de um grupo, a pilula vem para a frente das abas que
///   ficam (antes saltava-se para a aba solta seguinte e o grupo inteiro
///   sumia da barra);
/// - a pilula de um grupo que ficou inteiro fora do corte continua na barra
///   enquanto houver lugar, a mais proxima primeiro -- como no Chrome, onde
///   uma pilula nunca sai da faixa. Um clique nela mostra as abas.
///
/// Ancoras: `active` e a aba aberta ao lado (o Split) desta coluna, `focus`
/// a aba em que o dono acabou de mexer (largou-a, juntou-a a um grupo, pediu
/// para ver o grupo dela). Nenhuma sai da barra: nem quando o grupo dela esta
/// recolhido -- a aberta ao lado fica sozinha depois da pilula; a outra e
/// alcancada pela pilula --, nem quando o corte pelo fim a deixaria de fora.
/// Se as duas nao couberem juntas, ganha `focus`: e para ela que se olha.
pub(in crate::windows_app) fn plan_tab_row_focused(
    tabs: &[ContextTab],
    groups: &[ContextGroup],
    active: Option<u64>,
    focus: Option<u64>,
) -> TabRow {
    let group_of = |index: usize| -> Option<usize> {
        tabs.get(index)
            .and_then(|tab| tab.group)
            .and_then(|id| groups.iter().position(|group| group.id == id))
    };
    let active_index = active.and_then(|id| tabs.iter().position(|tab| tab.id == id));

    let mut full: Vec<TabSlot> = Vec::new();
    let mut billed: Vec<usize> = Vec::new();
    for index in 0..tabs.len() {
        if let Some(group) = group_of(index) {
            if !billed.contains(&group) {
                billed.push(group);
                full.push(TabSlot::Group(group));
            }
            // Recolhido esconde as abas do grupo, menos a que esta aberta ao
            // lado: essa continua ao alcance do rato.
            if groups[group].collapsed && Some(index) != active_index {
                continue;
            }
        }
        full.push(TabSlot::Tab(index));
    }

    // Onde esta cada ancora na fila completa. Uma aba escondida num grupo
    // recolhido e alcancada pela pilula dele.
    let place = |id: u64| -> Option<usize> {
        let index = tabs.iter().position(|tab| tab.id == id)?;
        full.iter()
            .position(|slot| *slot == TabSlot::Tab(index))
            .or_else(|| {
                let group = group_of(index)?;
                full.iter().position(|slot| *slot == TabSlot::Group(group))
            })
    };
    let mut anchors: Vec<usize> = Vec::new();
    for position in [focus, active].into_iter().flatten().filter_map(place) {
        if !anchors.contains(&position) {
            anchors.push(position);
        }
    }

    let window = full.len().checked_sub(1).map(|last| {
        let tail = row_window(&full, &group_of, last, last).unwrap_or((last, last + 1));
        if anchors.iter().all(|position| *position >= tail.0) {
            return tail;
        }
        let low = anchors.iter().copied().min().unwrap_or(last);
        let high = anchors.iter().copied().max().unwrap_or(last);
        row_window(&full, &group_of, low, high)
            .or_else(|| row_window(&full, &group_of, anchors[0], anchors[0]))
            .unwrap_or(tail)
    });

    let mut row = TabRow::empty();
    row.total_tabs = tabs.len();
    let Some((start, end)) = window else {
        return row;
    };
    let mut kept: Vec<TabSlot> = Vec::new();
    if let Some(group) = window_needs_chip(&full, &group_of, start) {
        kept.push(TabSlot::Group(group));
    }
    kept.extend_from_slice(&full[start..end]);

    // As pilulas dos grupos que ficaram inteiros fora do corte, as mais
    // proximas primeiro, enquanto houver lugar.
    let outside = |slot: &&TabSlot| matches!(slot, TabSlot::Group(_)) && !kept.contains(slot);
    let mut room = MAX_VISIBLE_TAB_SLOTS.saturating_sub(kept.len());
    let lead: Vec<TabSlot> = full[..start]
        .iter()
        .rev()
        .filter(outside)
        .take(room)
        .copied()
        .collect();
    room -= lead.len();
    let trail: Vec<TabSlot> = full[end..]
        .iter()
        .filter(outside)
        .take(room)
        .copied()
        .collect();
    for slot in lead.into_iter().rev().chain(kept).chain(trail) {
        row.push(slot);
    }

    let shown: Vec<usize> = row
        .visible()
        .iter()
        .filter_map(|slot| match slot {
            TabSlot::Tab(index) => Some(*index),
            TabSlot::Group(_) => None,
        })
        .collect();
    let pinned: Vec<usize> = [focus, active]
        .into_iter()
        .flatten()
        .filter_map(|id| tabs.iter().position(|tab| tab.id == id))
        .collect();
    for position in 0..row.len {
        match row.slots[position] {
            TabSlot::Tab(index) => {
                row.owners[position] = group_of(index);
                row.pinned[position] = pinned.contains(&index);
            }
            TabSlot::Group(group) => {
                row.behind[position] = (0..tabs.len())
                    .filter(|index| group_of(*index) == Some(group) && !shown.contains(index))
                    .count();
                row.collapsed[position] = groups[group].collapsed;
                row.pinned[position] = pinned
                    .iter()
                    .any(|index| group_of(*index) == Some(group) && !shown.contains(index));
            }
        }
    }
    row
}

/// O troco comeca a meio de um grupo (numa aba agrupada): a pilula dele fica
/// antes do troco e tem de vir para a frente.
pub(in crate::windows_app) fn window_needs_chip(
    full: &[TabSlot],
    group_of: &dyn Fn(usize) -> Option<usize>,
    start: usize,
) -> Option<usize> {
    match full.get(start)? {
        TabSlot::Tab(index) => group_of(*index),
        TabSlot::Group(_) => None,
    }
}

/// O troco `[inicio, fim)` da fila completa que cabe na barra e cobre as
/// posicoes `low..=high`: no maximo `MAX_VISIBLE_CONTEXT_TABS` abas e
/// `MAX_VISIBLE_TAB_SLOTS` lugares, contando a pilula que venha para a frente.
/// Cresce primeiro para a direita (as abas mais recentes), depois para a
/// esquerda. `None`: as posicoes nao cabem juntas.
pub(in crate::windows_app) fn row_window(
    full: &[TabSlot],
    group_of: &dyn Fn(usize) -> Option<usize>,
    low: usize,
    high: usize,
) -> Option<(usize, usize)> {
    let fits = |start: usize, end: usize| {
        let tabs = full[start..end]
            .iter()
            .filter(|slot| matches!(slot, TabSlot::Tab(_)))
            .count();
        let chip = usize::from(window_needs_chip(full, group_of, start).is_some());
        tabs <= MAX_VISIBLE_CONTEXT_TABS && end - start + chip <= MAX_VISIBLE_TAB_SLOTS
    };
    if low > high || high >= full.len() || !fits(low, high + 1) {
        return None;
    }
    let (mut start, mut end) = (low, high + 1);
    while end < full.len() && fits(start, end + 1) {
        end += 1;
    }
    while start > 0 && fits(start - 1, end) {
        start -= 1;
    }
    Some((start, end))
}

/// Cria um grupo com a aba indicada e devolve o indice do grupo novo. O nome
/// sai do host da aba -- um grupo sem nome nao diz nada a ninguem.
pub(in crate::windows_app) fn create_context_group(
    tabs: &mut [ContextTab],
    groups: &mut Vec<ContextGroup>,
    next_id: &mut u64,
    tab_index: usize,
) -> Option<usize> {
    let name = context_tab_label(&tabs.get(tab_index)?.url);
    let used: Vec<GroupColor> = groups.iter().map(|group| group.color).collect();
    let id = *next_id;
    *next_id += 1;
    groups.push(ContextGroup {
        id,
        name,
        color: GroupColor::next(&used),
        collapsed: false,
    });
    tabs[tab_index].group = Some(id);
    Some(groups.len() - 1)
}

/// Poe a aba no grupo e encosta-a ao ultimo membro: os membros de um grupo tem
/// de ficar juntos na barra, senao a pilula fica a rotular abas que nao sao
/// dela.
pub(in crate::windows_app) fn join_context_group(tabs: &mut Vec<ContextTab>, group_id: u64, tab_index: usize) {
    if tab_index >= tabs.len() {
        return;
    }
    let mut tab = tabs.remove(tab_index);
    tab.group = Some(group_id);
    let target = tabs
        .iter()
        .rposition(|other| other.group == Some(group_id))
        .map(|last| last + 1)
        .unwrap_or(tabs.len());
    tabs.insert(target, tab);
}

/// Tira a aba do grupo. Um grupo que fique sem abas desaparece -- uma pilula
/// vazia so ocupava espaco e enganava.
///
/// A aba sai para logo a seguir ao ultimo membro, como no Chrome. Solta-la no
/// sitio onde estava partia o grupo em dois quando ela estava no meio: a
/// pilula ficava a rotular metade e a outra metade parecia de ninguem.
pub(in crate::windows_app) fn leave_context_group(
    tabs: &mut Vec<ContextTab>,
    groups: &mut Vec<ContextGroup>,
    tab_index: usize,
) {
    let _ = detach_from_group(tabs, tab_index);
    prune_empty_groups(tabs, groups);
}

/// Solta a aba do seu grupo e poe-na logo depois do ultimo membro que fica.
/// Devolve onde a aba ficou. Uma aba solta fica onde esta.
pub(in crate::windows_app) fn detach_from_group(tabs: &mut Vec<ContextTab>, tab_index: usize) -> usize {
    let Some(group) = tabs.get(tab_index).and_then(|tab| tab.group) else {
        return tab_index;
    };
    let mut tab = tabs.remove(tab_index);
    tab.group = None;
    let target = tabs
        .iter()
        .rposition(|other| other.group == Some(group))
        .map_or(tab_index.min(tabs.len()), |last| last + 1);
    tabs.insert(target, tab);
    target
}

/// Cada grupo ocupa um troco seguido da fila. E a invariante que a barra
/// assume para desenhar a pilula e o sublinhado; todas as operacoes sobre as
/// abas a mantem, e os testes confirmam-no depois de cada uma.
#[cfg(test)]
pub(in crate::windows_app) fn group_runs_are_contiguous(tabs: &[ContextTab]) -> bool {
    let mut closed: Vec<u64> = Vec::new();
    let mut current: Option<u64> = None;
    for tab in tabs {
        if tab.group != current {
            if let Some(previous) = current {
                closed.push(previous);
            }
            if tab.group.is_some_and(|group| closed.contains(&group)) {
                return false;
            }
            current = tab.group;
        }
    }
    true
}

/// Onde comeca e onde acaba (exclusivo) o troco do grupo na fila.
pub(in crate::windows_app) fn group_run(tabs: &[ContextTab], group_id: u64) -> Option<(usize, usize)> {
    let start = tabs.iter().position(|tab| tab.group == Some(group_id))?;
    let end = tabs
        .iter()
        .rposition(|tab| tab.group == Some(group_id))
        .map_or(start + 1, |last| last + 1);
    Some((start, end))
}

/// Acerta o lugar `at` de uma aba que entra na fila (ja sem ela) com o grupo
/// `group`, para nao partir o troco de grupo nenhum: longe dos outros membros,
/// vai para o fim do troco do seu grupo; solta no meio de outro grupo, vai
/// para logo depois dele.
pub(in crate::windows_app) fn contiguous_slot(tabs: &[ContextTab], at: usize, group: Option<u64>) -> usize {
    let at = at.min(tabs.len());
    let before = at
        .checked_sub(1)
        .and_then(|index| tabs.get(index))
        .and_then(|tab| tab.group);
    let after = tabs.get(at).and_then(|tab| tab.group);
    if let Some(group) = group
        && let Some((_, end)) = group_run(tabs, group)
    {
        if before == Some(group) || after == Some(group) {
            return at;
        }
        return end;
    }
    match (before, after) {
        (Some(left), Some(right)) if left == right => {
            group_run(tabs, left).map_or(at, |(_, end)| end)
        }
        _ => at,
    }
}

/// Muda uma aba de lugar (e de grupo) na fila da sua coluna. `drop.before` e
/// o indice, na fila de ANTES da mudanca, da aba que fica a seguir a ela
/// (`None`: no fim). Um grupo que ja nao existe conta como "sem grupo".
pub(in crate::windows_app) fn move_context_tab(
    tabs: &mut Vec<ContextTab>,
    groups: &mut Vec<ContextGroup>,
    from: usize,
    drop: TabDrop,
) -> bool {
    if from >= tabs.len() {
        return false;
    }
    let group = drop
        .group
        .filter(|id| groups.iter().any(|group| group.id == *id));
    let mut tab = tabs.remove(from);
    let at = drop.before.map_or(
        tabs.len(),
        |before| {
            if before > from { before - 1 } else { before }
        },
    );
    let at = contiguous_slot(tabs, at, group);
    tab.group = group;
    tabs.insert(at, tab);
    prune_empty_groups(tabs, groups);
    true
}

/// Muda um grupo inteiro de lugar: o troco sai junto e entra antes da aba
/// `before` (indice na fila de antes; `None`: no fim), nunca no meio de outro
/// grupo.
pub(in crate::windows_app) fn move_context_group(
    tabs: &mut Vec<ContextTab>,
    group_id: u64,
    before: Option<usize>,
) -> bool {
    let Some((start, end)) = group_run(tabs, group_id) else {
        return false;
    };
    let run: Vec<ContextTab> = tabs.drain(start..end).collect();
    let at = before.map_or(tabs.len(), |before| {
        if before >= end {
            before - (end - start)
        } else {
            before.min(start)
        }
    });
    let at = contiguous_slot(tabs, at, None);
    for (offset, tab) in run.into_iter().enumerate() {
        tabs.insert(at + offset, tab);
    }
    true
}

pub(in crate::windows_app) fn prune_empty_groups(tabs: &[ContextTab], groups: &mut Vec<ContextGroup>) {
    groups.retain(|group| tabs.iter().any(|tab| tab.group == Some(group.id)));
}

/// Guarda uma nova aba de contexto e devolve a sua identidade estavel. Se a
/// ultima aba ja e a mesma URL, reutiliza-a; ao aplicar os limites
/// (`prune_context_tabs`, que nunca tira a aba nova), poda tambem o grupo que
/// eventualmente ficou sem o seu ultimo membro.
///
/// `opener` e a aba de onde o link saiu. Se ela estiver num grupo, a aba nova
/// nasce nesse grupo, no fim do seu troco -- como no Chrome, onde o que se
/// abre a partir de uma aba agrupada fica no grupo dela.
pub(in crate::windows_app) fn remember_context_tab(
    tabs: &mut Vec<ContextTab>,
    groups: &mut Vec<ContextGroup>,
    next_id: &mut u64,
    url: String,
    opener: Option<u64>,
) -> u64 {
    let opener_group = opener
        .and_then(|id| tabs.iter().find(|tab| tab.id == id))
        .and_then(|tab| tab.group)
        .filter(|id| groups.iter().any(|group| group.id == *id));
    let at = opener_group
        .and_then(|group| group_run(tabs, group))
        .map_or(tabs.len(), |(_, end)| end);
    if let Some(previous) = at.checked_sub(1).and_then(|index| tabs.get(index))
        && previous.url == url
        && (opener_group.is_none() || previous.group == opener_group)
    {
        return previous.id;
    }

    let id = *next_id;
    *next_id = (*next_id).wrapping_add(1).max(1);
    tabs.insert(
        at,
        ContextTab {
            id,
            url,
            group: opener_group,
        },
    );
    let _ = prune_context_tabs(tabs, groups, &[id]);
    id
}

/// Os limites de abas de uma coluna (`tab_session::prune_victims`, a mesma
/// regra do `tabs.json`): saem as soltas mais antigas -- pela identidade, que
/// sobe com cada aba nova, nunca pela posicao na barra --, uma agrupada so
/// quando ja nao ha soltas acima do tecto, e nunca as `protected` (a que
/// acabou de nascer, a aberta ao lado). Antes saia a primeira da esquerda: la
/// ficam os grupos que o dono fez primeiro, e com as abas a sobreviver a
/// pesquisas e reinicios o limite passou a apaga-los sem aviso.
pub(in crate::windows_app) fn prune_context_tabs(
    tabs: &mut Vec<ContextTab>,
    groups: &mut Vec<ContextGroup>,
    protected: &[u64],
) -> Vec<u64> {
    let ages: Vec<(u64, bool, bool)> = tabs
        .iter()
        .map(|tab| {
            (
                tab.id,
                tab.group
                    .is_some_and(|id| groups.iter().any(|group| group.id == id)),
                protected.contains(&tab.id),
            )
        })
        .collect();
    let victims = tab_session::prune_victims(&ages);
    let removed: Vec<u64> = victims.iter().map(|index| tabs[*index].id).collect();
    for index in victims.into_iter().rev() {
        tabs.remove(index);
    }
    if !removed.is_empty() {
        prune_empty_groups(tabs, groups);
    }
    removed
}

/// O aviso quando o tecto de abas tirou abas agrupadas (nunca em silencio).
pub(in crate::windows_app) fn lost_grouped_notice(lost: usize) -> Option<String> {
    match lost {
        0 => None,
        1 => Some(format!(
            "Limite de {} abas nesta IA: a aba agrupada mais antiga foi fechada.",
            tab_session::MAX_KEPT_TABS_PER_COLUMN
        )),
        _ => Some(format!(
            "Limite de {} abas nesta IA: as {lost} abas agrupadas mais antigas foram fechadas.",
            tab_session::MAX_KEPT_TABS_PER_COLUMN
        )),
    }
}

/// As identidades das abas agrupadas de uma coluna.
pub(in crate::windows_app) fn grouped_tab_ids(tabs: &[ContextTab]) -> Vec<u64> {
    tabs.iter()
        .filter(|tab| tab.group.is_some())
        .map(|tab| tab.id)
        .collect()
}

/// Quantas das abas agrupadas `before` ja nao estao na coluna. So o tecto
/// de abas tira uma agrupada, e isso o dono tem de saber.
pub(in crate::windows_app) fn lost_grouped_tabs(before: &[u64], tabs: &[ContextTab]) -> usize {
    before
        .iter()
        .filter(|id| !tabs.iter().any(|tab| tab.id == **id))
        .count()
}

/// Decide se uma coluna ainda pode ser minimizada sem esconder todas as IAs.
/// Esta decisao acontece ANTES de fechar um Split ativo: um clique rejeitado
/// nao pode destruir estado que o utilizador tinha aberto.
pub(in crate::windows_app) fn can_minimize_column(
    minimized: &[bool; COMPARATOR_COLUMNS],
    columns: usize,
    index: usize,
) -> bool {
    index < columns
        && !minimized[index]
        && (0..columns.min(COMPARATOR_COLUMNS))
            .filter(|slot| !minimized[*slot])
            .count()
            > 1
}

/// Fecha as outras abas do MESMO escopo da aba selecionada. Um grupo real
/// usa o seu id; abas soltas partilham o escopo `None`. Abas de outros grupos
/// nunca sao tocadas.
pub(in crate::windows_app) fn active_context_removed_by_scope(
    tabs: &[ContextTab],
    context_index: usize,
    active_id: Option<u64>,
    keep_selected: bool,
) -> bool {
    let Some(selected) = tabs.get(context_index) else {
        return false;
    };
    let Some(active_id) = active_id else {
        return false;
    };
    tabs.iter().any(|tab| {
        tab.id == active_id
            && tab.group == selected.group
            && (!keep_selected || tab.id != selected.id)
    })
}

pub(in crate::windows_app) fn close_other_context_tabs_in_scope(
    tabs: &mut Vec<ContextTab>,
    groups: &mut Vec<ContextGroup>,
    context_index: usize,
) -> bool {
    let Some(scope) = tabs.get(context_index).map(|tab| tab.group) else {
        return false;
    };
    let mut index = 0usize;
    tabs.retain(|tab| {
        let keep = index == context_index || tab.group != scope;
        index += 1;
        keep
    });
    prune_empty_groups(tabs, groups);
    true
}

/// Fecha todas as abas do escopo selecionado e preserva integralmente os
/// demais grupos da coluna.
pub(in crate::windows_app) fn close_context_tab_scope(
    tabs: &mut Vec<ContextTab>,
    groups: &mut Vec<ContextGroup>,
    context_index: usize,
) -> bool {
    let Some(scope) = tabs.get(context_index).map(|tab| tab.group) else {
        return false;
    };
    let before = tabs.len();
    tabs.retain(|tab| tab.group != scope);
    prune_empty_groups(tabs, groups);
    tabs.len() != before
}

/// Reagrupar uma aba pode esvaziar o grupo anterior. A criacao e a poda
/// pertencem a uma unica operacao para nunca deixar pilulas fantasmas.
///
/// Uma aba do meio de um grupo sai primeiro para depois do ultimo membro:
/// criar o grupo novo no sitio dela partia o antigo em dois.
pub(in crate::windows_app) fn regroup_context_tab(
    tabs: &mut Vec<ContextTab>,
    groups: &mut Vec<ContextGroup>,
    next_id: &mut u64,
    context_index: usize,
) -> Option<usize> {
    if context_index >= tabs.len() {
        return None;
    }
    let context_index = detach_from_group(tabs, context_index);
    let created = create_context_group(tabs, groups, next_id, context_index)?;
    let created_id = groups.get(created)?.id;
    prune_empty_groups(tabs, groups);
    groups.iter().position(|group| group.id == created_id)
}

/// O que o menu do botao direito sobre a pilula de um grupo faz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum GroupMenuCommand {
    Color(GroupColor),
    ToggleCollapsed,
    Ungroup,
    Close,
}

/// As cores ocupam `GROUP_MENU_COLOR_BASE + indice em GroupColor::ALL`.
pub(in crate::windows_app) const GROUP_MENU_COLOR_BASE: usize = 200;
pub(in crate::windows_app) const GROUP_MENU_TOGGLE: usize = 220;
pub(in crate::windows_app) const GROUP_MENU_UNGROUP: usize = 221;
pub(in crate::windows_app) const GROUP_MENU_CLOSE: usize = 222;

/// Id devolvido pelo `TrackPopupMenu` do grupo -> operacao. Zero (menu
/// fechado sem escolha) e ids fora da lista nao fazem nada.
pub(in crate::windows_app) fn group_menu_command(id: usize) -> Option<GroupMenuCommand> {
    match id {
        GROUP_MENU_TOGGLE => Some(GroupMenuCommand::ToggleCollapsed),
        GROUP_MENU_UNGROUP => Some(GroupMenuCommand::Ungroup),
        GROUP_MENU_CLOSE => Some(GroupMenuCommand::Close),
        _ => id
            .checked_sub(GROUP_MENU_COLOR_BASE)
            .and_then(|index| GroupColor::ALL.get(index))
            .copied()
            .map(GroupMenuCommand::Color),
    }
}

/// Aplica o comando ao grupo `group_id` da coluna e devolve as identidades
/// das abas que fecharam: se a que esta aberta ao lado estiver entre elas, o
/// App fecha o Split.
pub(in crate::windows_app) fn apply_group_command(
    tabs: &mut Vec<ContextTab>,
    groups: &mut Vec<ContextGroup>,
    group_id: u64,
    command: GroupMenuCommand,
) -> Vec<u64> {
    let Some(position) = groups.iter().position(|group| group.id == group_id) else {
        return Vec::new();
    };
    match command {
        GroupMenuCommand::Color(color) => {
            groups[position].color = color;
            Vec::new()
        }
        GroupMenuCommand::ToggleCollapsed => {
            groups[position].collapsed = !groups[position].collapsed;
            Vec::new()
        }
        // As abas ficam onde estao, soltas: o troco era seguido, continua.
        GroupMenuCommand::Ungroup => {
            for tab in tabs.iter_mut().filter(|tab| tab.group == Some(group_id)) {
                tab.group = None;
            }
            prune_empty_groups(tabs, groups);
            Vec::new()
        }
        GroupMenuCommand::Close => {
            let closed: Vec<u64> = tabs
                .iter()
                .filter(|tab| tab.group == Some(group_id))
                .map(|tab| tab.id)
                .collect();
            tabs.retain(|tab| tab.group != Some(group_id));
            prune_empty_groups(tabs, groups);
            closed
        }
    }
}

/// O que o clique (botao esquerdo) na pilula de um grupo faz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum ChipClick {
    /// Recolhido: abre, e as abas dele passam a estar a vista.
    Expand,
    /// Aberto, mas nenhuma aba dele esta a vista (ficaram fora do corte e a
    /// pilula esta sozinha, com cara de recolhida): mostra-as.
    Reveal,
    /// Aberto e com abas a vista: recolhe.
    Collapse,
}

pub(in crate::windows_app) fn chip_click(collapsed: bool, members_drawn: bool) -> ChipClick {
    match (collapsed, members_drawn) {
        (true, _) => ChipClick::Expand,
        (false, false) => ChipClick::Reveal,
        (false, true) => ChipClick::Collapse,
    }
}

/// Aplica o clique na pilula `group_index`. Mostrar as abas e pôr a primeira
/// do grupo como ancora da coluna (`focus`): a barra corta a fila a volta
/// dela, pilula incluida.
pub(in crate::windows_app) fn apply_chip_click(
    tabs: &[ContextTab],
    groups: &mut [ContextGroup],
    focus: &mut Option<u64>,
    group_index: usize,
    members_drawn: bool,
) -> Option<ChipClick> {
    let group = groups.get_mut(group_index)?;
    let click = chip_click(group.collapsed, members_drawn);
    let first = tabs
        .iter()
        .find(|tab| tab.group == Some(group.id))
        .map(|tab| tab.id);
    match click {
        ChipClick::Expand => {
            group.collapsed = false;
            *focus = first.or(*focus);
        }
        ChipClick::Reveal => *focus = first.or(*focus),
        ChipClick::Collapse => group.collapsed = true,
    }
    Some(click)
}

/// Uma linha da lista de todas as abas de uma coluna (o "‹N").
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::windows_app) enum TabListEntry {
    /// Uma aba solta: indice na coluna e rotulo.
    Tab { index: usize, label: String },
    /// Um grupo, com as suas abas (indice na coluna e rotulo).
    Group {
        group: usize,
        name: String,
        tabs: Vec<(usize, String)>,
    },
}

/// Todas as abas da coluna pela ordem da barra, cada grupo com as suas --
/// recolhidos e cortados incluidos. E por aqui que qualquer aba guardada no
/// `tabs.json` volta a estar ao alcance, por mais antiga que seja.
pub(in crate::windows_app) fn tab_list_entries(tabs: &[ContextTab], groups: &[ContextGroup]) -> Vec<TabListEntry> {
    let mut entries: Vec<TabListEntry> = Vec::new();
    for (index, tab) in tabs.iter().enumerate() {
        let label = context_tab_label(&tab.url);
        let group = tab
            .group
            .and_then(|id| groups.iter().position(|group| group.id == id));
        match group {
            None => entries.push(TabListEntry::Tab { index, label }),
            Some(group) => {
                if let Some(TabListEntry::Group {
                    group: last, tabs, ..
                }) = entries.last_mut()
                    && *last == group
                {
                    tabs.push((index, label));
                } else {
                    entries.push(TabListEntry::Group {
                        group,
                        name: groups[group].name.clone(),
                        tabs: vec![(index, label)],
                    });
                }
            }
        }
    }
    entries
}

/// Ids da lista de abas: `TAB_LIST_BASE + indice da aba na coluna`. Zero e o
/// "fechou sem escolher".
pub(in crate::windows_app) const TAB_LIST_BASE: usize = 1;

pub(in crate::windows_app) fn tab_list_command(id: usize, tabs: usize) -> Option<usize> {
    id.checked_sub(TAB_LIST_BASE).filter(|index| *index < tabs)
}

/// O que o menu do botao direito sobre uma aba faz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum TabMenuCommand {
    Open,
    Fullscreen,
    Close,
    CloseOthers,
    CloseAll,
    NewGroup,
    Ungroup,
    /// Indice (na coluna) do grupo escolhido em "Mover para o grupo".
    MoveToGroup(usize),
    /// "Cor do grupo" numa aba agrupada: a cor do grupo DELA, sem ter de ir
    /// a pilula -- os mesmos ids do menu do grupo.
    GroupColor(GroupColor),
}

/// Id devolvido pelo `TrackPopupMenu` da aba -> operacao. As entradas do
/// submenu "Mover para o grupo" sao `TAB_MENU_GROUP_BASE + posicao` em
/// `joinable`, a mesma lista com que o menu foi montado.
pub(in crate::windows_app) fn tab_menu_command(id: usize, joinable: &[(usize, String)]) -> Option<TabMenuCommand> {
    Some(match id {
        TAB_MENU_OPEN => TabMenuCommand::Open,
        TAB_MENU_FULLSCREEN => TabMenuCommand::Fullscreen,
        TAB_MENU_CLOSE => TabMenuCommand::Close,
        TAB_MENU_CLOSE_OTHERS => TabMenuCommand::CloseOthers,
        TAB_MENU_CLOSE_ALL => TabMenuCommand::CloseAll,
        TAB_MENU_NEW_GROUP => TabMenuCommand::NewGroup,
        TAB_MENU_UNGROUP => TabMenuCommand::Ungroup,
        _ => {
            if let Some(GroupMenuCommand::Color(color)) = group_menu_command(id) {
                return Some(TabMenuCommand::GroupColor(color));
            }
            let offset = id.checked_sub(TAB_MENU_GROUP_BASE)?;
            TabMenuCommand::MoveToGroup(joinable.get(offset)?.0)
        }
    })
}

/// O nome de cada cor no menu do grupo, como o Chrome em portugues.
pub(in crate::windows_app) fn group_color_label(color: GroupColor) -> &'static str {
    match color {
        GroupColor::Blue => "Azul",
        GroupColor::Green => "Verde",
        GroupColor::Amber => "Amarelo",
        GroupColor::Pink => "Rosa",
        GroupColor::Purple => "Roxo",
        GroupColor::Slate => "Cinza",
        GroupColor::Red => "Vermelho",
        GroupColor::Cyan => "Ciano",
        GroupColor::Orange => "Laranja",
    }
}

/// Onde fica uma aba largada: antes de que aba da fila (indice de ANTES da
/// mudanca; `None` = no fim) e em que grupo (`None` = solta).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) struct TabDrop {
    pub(in crate::windows_app) before: Option<usize>,
    pub(in crate::windows_app) group: Option<u64>,
}

/// O que se arrasta na barra, pela identidade estavel: os indices podem mudar
/// a meio do gesto se chegar uma aba nova.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum DragItem {
    Tab(u64),
    Group(u64),
}

/// Onde o que se arrasta vai cair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum DropSpot {
    Tab(TabDrop),
    /// O troco inteiro entra antes da aba `before` (indice de antes).
    Group {
        before: Option<usize>,
    },
}

/// Um lugar da fila de uma coluna tal como esta desenhado.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::windows_app) struct RowItem {
    pub(in crate::windows_app) kind: RowKind,
    pub(in crate::windows_app) rect: UiRect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum RowKind {
    /// A pilula do grupo com este indice na coluna.
    Chip(usize),
    /// A aba com este indice na coluna, e o indice do grupo dela.
    Tab {
        context: usize,
        owner: Option<usize>,
    },
}

/// Quanto o rato tem de andar com o botao em baixo, PARA ALEM disto, antes de
/// o gesto deixar de ser um clique e passar a ser um arrasto (pixeis
/// logicos). Ate 4 px e a mao a tremer num clique.
pub(in crate::windows_app) const DRAG_THRESHOLD: f64 = 4.0;
/// Folga, alem das pontas da fila da coluna, onde ainda se pode largar.
pub(in crate::windows_app) const DROP_MARGIN: f64 = 24.0;

pub(in crate::windows_app) fn drag_started(origin: (f64, f64), now: (f64, f64), scale: f64) -> bool {
    let limit = DRAG_THRESHOLD * scale.max(1.0);
    (now.0 - origin.0).abs() > limit || (now.1 - origin.1).abs() > limit
}

/// Se o lugar `kind` da fila e o que se arrasta: a propria aba, ou a pilula e
/// os membros do grupo arrastado.
pub(in crate::windows_app) fn row_item_is_dragged(
    kind: RowKind,
    item: DragItem,
    tabs: &[ContextTab],
    groups: &[ContextGroup],
) -> bool {
    let group_id = |index: usize| groups.get(index).map(|group| group.id);
    match (item, kind) {
        (DragItem::Tab(id), RowKind::Tab { context, .. }) => {
            tabs.get(context).is_some_and(|tab| tab.id == id)
        }
        (DragItem::Group(id), RowKind::Chip(group))
        | (
            DragItem::Group(id),
            RowKind::Tab {
                owner: Some(group), ..
            },
        ) => group_id(group) == Some(id),
        _ => false,
    }
}

/// A faixa onde largar ainda e largar na fila da coluna: a propria fila, com
/// uma folga nas pontas e por baixo das abas. A folga nunca invade a fila de
/// outra IA -- as abas sao da IA desta coluna e nao mudam de IA. Fora daqui,
/// largar cancela o arrasto.
pub(in crate::windows_app) fn in_drop_zone(
    layout: &BarLayout,
    column: usize,
    cursor: (f64, f64),
    scale: f64,
) -> bool {
    let scale = scale.max(1.0);
    let items = layout.row_items(column);
    let (Some(first), Some(last)) = (items.first(), items.last()) else {
        return false;
    };
    let (x, y) = cursor;
    let margin = DROP_MARGIN * scale;
    let mut low = first.rect.x - margin;
    let mut high = last.rect.x + last.rect.width + margin;
    if let Some(previous) = (0..column)
        .rev()
        .find_map(|index| layout.row_items(index).last().copied())
    {
        low = low.max(previous.rect.x + previous.rect.width);
    }
    if let Some(next) =
        (column + 1..layout.columns_len).find_map(|index| layout.row_items(index).first().copied())
    {
        high = high.min(next.rect.x);
    }
    y >= 0.0 && y <= first.rect.y + first.rect.height + margin && x >= low && x < high
}

/// Onde cai o que se arrasta com o rato em `cursor`, contado contra a fila
/// tal como esta desenhada ANTES do arrasto (o modelo so muda ao largar, por
/// isso esta conta nao anda aos saltos com a pre-visualizacao). `None`:
/// largar ali nao muda nada -- em cima de si proprio, ou fora da faixa da
/// coluna (`in_drop_zone`).
///
/// Regras, como no Chrome: entre dois membros de um grupo, ou sobre a metade
/// direita de um membro ou da pilula aberta, a aba entra no grupo; antes de
/// uma pilula, depois do ultimo membro ou sobre uma aba solta, fica solta --
/// e uma aba de um grupo levada para fora do troco sai dele. Um grupo nunca
/// cai no meio de outro: encosta-se ao lado mais proximo.
pub(in crate::windows_app) fn plan_drop(
    layout: &BarLayout,
    column: usize,
    tabs: &[ContextTab],
    groups: &[ContextGroup],
    item: DragItem,
    cursor: (f64, f64),
    scale: f64,
) -> Option<DropSpot> {
    if !in_drop_zone(layout, column, cursor, scale) {
        return None;
    }
    let items = layout.row_items(column);
    let x = cursor.0;
    let group_id = |index: usize| groups.get(index).map(|group| group.id);
    let own = |kind: RowKind| row_item_is_dragged(kind, item, tabs, groups);
    let mut own_span: Option<(f64, f64)> = None;
    for entry in items.iter().filter(|entry| own(entry.kind)) {
        let right = entry.rect.x + entry.rect.width;
        own_span = Some(own_span.map_or((entry.rect.x, right), |(left, end)| {
            (left.min(entry.rect.x), end.max(right))
        }));
    }
    if own_span.is_some_and(|(left, right)| x >= left && x < right) {
        return None;
    }
    let others: Vec<RowItem> = items
        .iter()
        .copied()
        .filter(|entry| !own(entry.kind))
        .collect();
    if others.is_empty() {
        return None;
    }

    let gap_index = others
        .iter()
        .filter(|entry| entry.rect.x + entry.rect.width / 2.0 < x)
        .count();
    let left = gap_index.checked_sub(1).and_then(|index| others.get(index));
    let right = others.get(gap_index);
    let attach_left = left.is_some_and(|entry| x < entry.rect.x + entry.rect.width);
    let run = |group: usize| group_id(group).and_then(|id| group_run(tabs, id));
    let collapsed = |group: usize| groups.get(group).is_some_and(|group| group.collapsed);

    Some(match item {
        DragItem::Tab(_) => {
            // A aba aberta de um grupo recolhido e a unica dele a vista:
            // largar sobre ela entrava no grupo recolhido e a aba sumia.
            // Resolve-se como a pilula recolhida: logo depois do troco, solta.
            let target = if attach_left { left } else { right };
            let hidden_group = target.and_then(|entry| match entry.kind {
                RowKind::Tab {
                    owner: Some(group), ..
                } if collapsed(group) => Some(group),
                _ => None,
            });
            let (before, join) = match (attach_left, left, right) {
                _ if hidden_group.is_some() => (
                    hidden_group.and_then(|group| run(group).map(|(_, end)| end)),
                    None,
                ),
                (true, Some(entry), _) => match entry.kind {
                    RowKind::Tab { context, owner } => (Some(context + 1), owner),
                    RowKind::Chip(group) if collapsed(group) => {
                        (run(group).map(|(_, end)| end), None)
                    }
                    RowKind::Chip(group) => (run(group).map(|(start, _)| start), Some(group)),
                },
                (_, _, Some(entry)) => match entry.kind {
                    RowKind::Tab { context, owner } => (Some(context), owner),
                    RowKind::Chip(group) => (run(group).map(|(start, _)| start), None),
                },
                _ => (None, None),
            };
            DropSpot::Tab(TabDrop {
                before,
                group: join.and_then(group_id),
            })
        }
        DragItem::Group(_) => {
            let before = match (attach_left, left, right) {
                (true, Some(entry), _) => match entry.kind {
                    RowKind::Tab {
                        owner: Some(group), ..
                    } => run(group).map(|(_, end)| end),
                    RowKind::Tab { context, .. } => Some(context + 1),
                    RowKind::Chip(group) if collapsed(group) => run(group).map(|(_, end)| end),
                    RowKind::Chip(group) => run(group).map(|(start, _)| start),
                },
                (_, _, Some(entry)) => match entry.kind {
                    RowKind::Tab {
                        context,
                        owner: Some(group),
                    } => run(group).map(|(start, end)| if context == start { start } else { end }),
                    RowKind::Tab { context, .. } => Some(context),
                    RowKind::Chip(group) => run(group).map(|(start, _)| start),
                },
                _ => None,
            };
            DropSpot::Group { before }
        }
    })
}

/// Aplica a largada ao modelo da coluna. Devolve `false` se nada mudou de
/// sitio (o item ja nao existe, ou o sitio e de outro tipo de item).
pub(in crate::windows_app) fn apply_drop(
    tabs: &mut Vec<ContextTab>,
    groups: &mut Vec<ContextGroup>,
    item: DragItem,
    spot: DropSpot,
) -> bool {
    match (item, spot) {
        (DragItem::Tab(id), DropSpot::Tab(drop)) => tabs
            .iter()
            .position(|tab| tab.id == id)
            .is_some_and(|from| move_context_tab(tabs, groups, from, drop)),
        (DragItem::Group(id), DropSpot::Group { before }) => move_context_group(tabs, id, before),
        _ => false,
    }
}

/// A fila da coluna como fica se o que se arrasta cair em `spot`: e o que a
/// barra desenha a meio do arrasto, com as outras abas ja a abrir-lhe lugar.
/// Sem sitio (fora da faixa, ou em cima de si proprio) e a fila de sempre.
/// Trabalha numa copia: o modelo so muda ao largar, e por isso o Esc, a
/// captura perdida e o largar fora da fila deixam tudo como estava sem ter
/// de desfazer nada.
pub(in crate::windows_app) fn drag_preview(
    tabs: &[ContextTab],
    groups: &[ContextGroup],
    item: DragItem,
    spot: Option<DropSpot>,
) -> (Vec<ContextTab>, Vec<ContextGroup>) {
    let mut tabs = tabs.to_vec();
    let mut groups = groups.to_vec();
    if let Some(spot) = spot {
        let _ = apply_drop(&mut tabs, &mut groups, item, spot);
    }
    (tabs, groups)
}

/// O modelo que a barra desenha a meio de um arrasto: as colunas com a coluna
/// do arrasto ja como ficara ao largar (`drag_preview`), e o que se arrasta
/// como ancora dela -- fica sempre na fila, mesmo quando cai longe das abas
/// mais recentes (como no Chrome), em vez de sumir da pre-visualizacao.
#[allow(clippy::type_complexity)]
pub(in crate::windows_app) fn drag_preview_model(
    contexts: &[Vec<ContextTab>; COMPARATOR_COLUMNS],
    groups: &[Vec<ContextGroup>; COMPARATOR_COLUMNS],
    focus: [Option<u64>; COMPARATOR_COLUMNS],
    drag: DragPaint,
) -> Option<(
    [Vec<ContextTab>; COMPARATOR_COLUMNS],
    [Vec<ContextGroup>; COMPARATOR_COLUMNS],
    [Option<u64>; COMPARATOR_COLUMNS],
)> {
    let column = drag.source_index;
    if column >= COMPARATOR_COLUMNS {
        return None;
    }
    let mut all_tabs = contexts.clone();
    let mut all_groups = groups.clone();
    let mut focus = focus;
    let (tabs, column_groups) =
        drag_preview(&contexts[column], &groups[column], drag.item, drag.spot);
    focus[column] = drag_focus(&tabs, drag.item);
    all_tabs[column] = tabs;
    all_groups[column] = column_groups;
    Some((all_tabs, all_groups, focus))
}

/// Quanto o que se arrasta sai do seu lugar na fila desenhada (`layout` ja e
/// a pre-visualizacao) para a borda esquerda ficar em `float_left` -- o rato
/// menos o ponto por onde foi agarrado --, sem sair da fila da sua coluna.
pub(in crate::windows_app) fn drag_float_offset(
    layout: &BarLayout,
    column: usize,
    tabs: &[ContextTab],
    groups: &[ContextGroup],
    item: DragItem,
    float_left: f64,
) -> Option<f64> {
    let items = layout.row_items(column);
    let (first, last) = (items.first()?, items.last()?);
    let mut block: Option<(f64, f64)> = None;
    for entry in items
        .iter()
        .filter(|entry| row_item_is_dragged(entry.kind, item, tabs, groups))
    {
        let right = entry.rect.x + entry.rect.width;
        block = Some(block.map_or((entry.rect.x, right), |(left, end)| {
            (left.min(entry.rect.x), end.max(right))
        }));
    }
    let (left, right) = block?;
    let row_left = first.rect.x;
    let row_right = last.rect.x + last.rect.width;
    let wanted = float_left.clamp(row_left, (row_right - (right - left)).max(row_left));
    Some(wanted - left)
}

/// Fecha a aba `index` da coluna (o x dela ou "Fechar aba" no menu) e poda o
/// grupo que fique vazio. Devolve a identidade da aba fechada, para o App
/// saber se era a que estava aberta ao lado. A coluna nunca fica sem nada: a
/// IA dela continua la, as abas sao so o que se abriu a partir dela.
pub(in crate::windows_app) fn remove_context_tab(
    tabs: &mut Vec<ContextTab>,
    groups: &mut Vec<ContextGroup>,
    index: usize,
) -> Option<u64> {
    if index >= tabs.len() {
        return None;
    }
    let closed = tabs.remove(index);
    prune_empty_groups(tabs, groups);
    Some(closed.id)
}

/// Botao esquerdo em baixo sobre um alvo da fila de abas (aba, x ou pilula).
/// O clique so se decide ao largar: ate la o gesto pode virar arrasto.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::windows_app) struct TabPress {
    /// Onde o botao desceu, em pixeis do cliente.
    pub(in crate::windows_app) origin: (f64, f64),
    /// O alvo sob o rato quando o botao desceu.
    pub(in crate::windows_app) hit: BarHit,
    /// A coluna e o que se arrasta se o rato andar; `None` no x, que nao se
    /// arrasta (como no Chrome).
    pub(in crate::windows_app) drag: Option<(usize, DragItem)>,
    /// Distancia do rato a borda esquerda do que foi premido (a aba, ou a
    /// pilula do grupo): o arrastado segue o rato agarrado por este ponto.
    pub(in crate::windows_app) anchor: f64,
    /// Numero deste gesto (nunca 0). O subclass da janela so ve estaticos; e
    /// por este numero que um WM_CAPTURECHANGED que chega atrasado se
    /// reconhece como de um gesto que ja acabou.
    pub(in crate::windows_app) gesture: u64,
    /// Ja passou o limiar: e um arrasto, ja nao e um clique.
    pub(in crate::windows_app) dragging: bool,
}

/// O premir do botao esquerdo em `origin`, sobre `hit` da barra `layout`.
/// So abas, o x delas e as pilulas esperam pelo largar; o resto nao e um
/// gesto da fila e responde logo.
pub(in crate::windows_app) fn tab_press(
    layout: &BarLayout,
    contexts: &[Vec<ContextTab>; COMPARATOR_COLUMNS],
    groups: &[Vec<ContextGroup>; COMPARATOR_COLUMNS],
    hit: BarHit,
    origin: (f64, f64),
    gesture: u64,
) -> Option<TabPress> {
    let drag = match hit {
        BarHit::ContextTab {
            source_index,
            context_index,
        } => Some((
            source_index,
            DragItem::Tab(contexts.get(source_index)?.get(context_index)?.id),
        )),
        BarHit::ContextGroup {
            source_index,
            group_index,
        } => Some((
            source_index,
            DragItem::Group(groups.get(source_index)?.get(group_index)?.id),
        )),
        BarHit::CloseTab { .. } => None,
        _ => return None,
    };
    let anchor = drag
        .and_then(|(column, item)| {
            layout.row_items(column).into_iter().find(|entry| {
                row_item_is_dragged(entry.kind, item, &contexts[column], &groups[column])
            })
        })
        .map_or(0.0, |entry| origin.0 - entry.rect.x);
    Some(TabPress {
        origin,
        hit,
        drag,
        anchor,
        gesture,
        dragging: false,
    })
}

/// O que o largar do botao esquerdo faz depois de um `TabPress`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum TabRelease {
    /// Premido e largado no mesmo alvo: e um clique nele.
    Click(BarHit),
    /// Fim de um arrasto: o item cai onde o rato esta.
    Drop { source_index: usize, item: DragItem },
    /// Largado noutro sitio sem arrastar: nada. Carregar no x e fugir com o
    /// rato nao fecha a aba -- a mesma regra dos botoes da janela.
    Nothing,
}

pub(in crate::windows_app) fn tab_release(press: TabPress, released: Option<BarHit>) -> TabRelease {
    if press.dragging {
        return press
            .drag
            .map_or(TabRelease::Nothing, |(source_index, item)| {
                TabRelease::Drop { source_index, item }
            });
    }
    if released == Some(press.hit) {
        TabRelease::Click(press.hit)
    } else {
        TabRelease::Nothing
    }
}

/// A fila de abas tal como o gesto a ve: a barra desenhada e o modelo das
/// colunas de onde ela saiu.
pub(in crate::windows_app) struct TabRowView<'a> {
    pub(in crate::windows_app) layout: &'a BarLayout,
    pub(in crate::windows_app) contexts: &'a [Vec<ContextTab>; COMPARATOR_COLUMNS],
    pub(in crate::windows_app) groups: &'a [Vec<ContextGroup>; COMPARATOR_COLUMNS],
    pub(in crate::windows_app) scale: f64,
}

impl TabRowView<'_> {
    pub(in crate::windows_app) fn plan(
        &self,
        column: usize,
        item: DragItem,
        cursor: (f64, f64),
    ) -> Option<DropSpot> {
        plan_drop(
            self.layout,
            column,
            self.contexts.get(column)?,
            self.groups.get(column)?,
            item,
            cursor,
            self.scale,
        )
    }
}

/// O que chega ao gesto sobre a fila de abas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::windows_app) enum TabGestureInput {
    /// O rato andou; `button_down` e o botao esquerdo tal como a fila de
    /// mensagens o ve.
    Move {
        cursor: (f64, f64),
        button_down: bool,
    },
    /// O botao esquerdo subiu com o rato em `cursor`, sobre `hit`.
    Release {
        cursor: (f64, f64),
        hit: Option<BarHit>,
    },
    /// Esc -- na janela, ou o "voltar" que a pagina manda quando o teclado
    /// esta nela.
    Escape,
    /// Outra janela ficou com o rato (WM_CAPTURECHANGED) durante o gesto
    /// numero `gesture`.
    CaptureLost { gesture: u64 },
}

/// O que o App faz depois de um passo do gesto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::windows_app) enum TabGestureEffect {
    /// Nao ha gesto, ou isto nao e com ele: o Esc volta a ser o "voltar".
    Ignored,
    /// Botao em baixo, ainda dentro do limiar: pode ser um clique.
    Pending,
    /// Passou o limiar agora: o rato fica preso a janela e a barra passa a
    /// desenhar a pre-visualizacao.
    Started,
    /// O arrasto continua: redesenhar a pre-visualizacao.
    Moved,
    /// Premido e largado no mesmo alvo, sem arrastar.
    Click(BarHit),
    /// Largado dentro da fila: e so aqui que o modelo muda. Nenhuma pagina
    /// navega nem recarrega -- muda a ordem e o grupo, mais nada.
    Drop {
        source_index: usize,
        item: DragItem,
        spot: DropSpot,
    },
    /// Acabou sem mudar nada: Esc, captura perdida, botao solto noutro sitio
    /// ou largado fora da fila.
    Cancelled { was_dragging: bool },
}

/// A maquina do gesto sobre a fila de abas: premir (`tab_press`), limiar,
/// alvo, largar, Esc e captura perdida. Pura -- recebe a fila desenhada e o
/// modelo, devolve o que o App faz --, e e por aqui que o App passa em cada
/// evento. O largar TIRA o gesto antes de mais nada: o ReleaseCapture que se
/// lhe segue manda um WM_CAPTURECHANGED, e esse ja nao encontra nada.
pub(in crate::windows_app) fn tab_gesture_step(
    press: &mut Option<TabPress>,
    input: TabGestureInput,
    row: &TabRowView,
) -> TabGestureEffect {
    let Some(current) = *press else {
        return TabGestureEffect::Ignored;
    };
    match input {
        TabGestureInput::Move {
            cursor,
            button_down,
        } => {
            // Sem o botao em baixo, o largar perdeu-se (foi para outra
            // janela): o gesto acaba aqui sem fazer nada.
            if !button_down {
                *press = None;
                return TabGestureEffect::Cancelled {
                    was_dragging: current.dragging,
                };
            }
            if current.dragging {
                return TabGestureEffect::Moved;
            }
            if current.drag.is_some() && drag_started(current.origin, cursor, row.scale) {
                *press = Some(TabPress {
                    dragging: true,
                    ..current
                });
                return TabGestureEffect::Started;
            }
            TabGestureEffect::Pending
        }
        TabGestureInput::Release { cursor, hit } => {
            *press = None;
            match tab_release(current, hit) {
                TabRelease::Click(hit) => TabGestureEffect::Click(hit),
                TabRelease::Drop { source_index, item } => {
                    match row.plan(source_index, item, cursor) {
                        Some(spot) => TabGestureEffect::Drop {
                            source_index,
                            item,
                            spot,
                        },
                        None => TabGestureEffect::Cancelled { was_dragging: true },
                    }
                }
                TabRelease::Nothing => TabGestureEffect::Cancelled {
                    was_dragging: current.dragging,
                },
            }
        }
        // Esc a meio de um arrasto cancela-o, como no Chrome, em vez do
        // "voltar" de sempre -- que podia sair da coluna expandida ou ir para
        // a Home com o botao ainda em baixo. Antes do limiar ainda e um
        // clique, e o Esc continua a ser o "voltar".
        TabGestureInput::Escape if current.dragging => {
            *press = None;
            TabGestureEffect::Cancelled { was_dragging: true }
        }
        TabGestureInput::Escape => TabGestureEffect::Ignored,
        TabGestureInput::CaptureLost { gesture } if gesture == current.gesture => {
            *press = None;
            TabGestureEffect::Cancelled {
                was_dragging: current.dragging,
            }
        }
        TabGestureInput::CaptureLost { .. } => TabGestureEffect::Ignored,
    }
}

/// O que um passo do gesto faz ao modelo das colunas: so o largar dentro da
/// fila o muda. Devolve se alguma aba mudou de sitio ou de grupo. O que foi
/// largado passa a ser a ancora da coluna (`focus`): fica na barra onde o
/// dono o pos, mesmo longe das abas mais recentes.
pub(in crate::windows_app) fn apply_tab_gesture(
    contexts: &mut [Vec<ContextTab>; COMPARATOR_COLUMNS],
    groups: &mut [Vec<ContextGroup>; COMPARATOR_COLUMNS],
    focus: &mut [Option<u64>; COMPARATOR_COLUMNS],
    effect: TabGestureEffect,
) -> bool {
    match effect {
        TabGestureEffect::Drop {
            source_index,
            item,
            spot,
        } if source_index < COMPARATOR_COLUMNS => {
            let moved = apply_drop(
                &mut contexts[source_index],
                &mut groups[source_index],
                item,
                spot,
            );
            if moved {
                focus[source_index] = drag_focus(&contexts[source_index], item);
            }
            moved
        }
        _ => false,
    }
}

/// O que a barra desenha enquanto se arrasta: a fila ja reordenada
/// (`drag_preview` com `spot`) e o que se arrasta a seguir o rato por cima
/// dela. Fora da faixa onde se larga (`float_left` a `None`) o item volta ao
/// seu lugar, esbatido: largar ali cancela.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::windows_app) struct DragPaint {
    pub(in crate::windows_app) source_index: usize,
    pub(in crate::windows_app) item: DragItem,
    pub(in crate::windows_app) spot: Option<DropSpot>,
    pub(in crate::windows_app) float_left: Option<f64>,
}

pub(in crate::windows_app) fn tab_drag_paint(
    press: TabPress,
    row: &TabRowView,
    cursor: (f64, f64),
) -> Option<DragPaint> {
    if !press.dragging {
        return None;
    }
    let (source_index, item) = press.drag?;
    let inside = in_drop_zone(row.layout, source_index, cursor, row.scale);
    Some(DragPaint {
        source_index,
        item,
        spot: row.plan(source_index, item, cursor),
        float_left: inside.then_some(cursor.0 - press.anchor),
    })
}

/// O gesto da fila de abas que tem o rato preso (0: nenhum). O App publica-o
/// a cada passo; o subclass da janela le-o no WM_CAPTURECHANGED.
pub(in crate::windows_app) static TAB_GESTURE_LIVE: AtomicU64 = AtomicU64::new(0);

/// WM_CAPTURECHANGED em `hwnd`: `new_owner` (nulo quando ninguem) ficou com o
/// rato. Havendo um gesto vivo, devolve o numero dele -- o App cancela-o se
/// ainda for o mesmo quando o aviso chegar. O SetCapture sobre quem ja tinha
/// o rato tambem manda esta mensagem, e ai nada se perdeu.
pub(in crate::windows_app) fn tab_gesture_capture_lost(
    live: &AtomicU64,
    hwnd: HWND,
    new_owner: HWND,
) -> Option<u64> {
    if new_owner == hwnd {
        return None;
    }
    let gesture = live.load(Ordering::Acquire);
    (gesture != 0).then_some(gesture)
}
