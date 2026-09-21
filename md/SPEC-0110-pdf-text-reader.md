# SPEC-0110 — Leitura textual de PDF

**Status:** Proposta com implementação neste PR; só vira implementada após gate verde.

## Objetivo

O viewer PDF já renderiza documentos offline. Esta spec acrescenta leitura
textual: texto selecionável nas páginas e conteúdo real do PDF na memória
semântica/pesquisa do NeuralIA.

## Limites

A extração automática é deliberadamente limitada a:

- 24 páginas por documento;
- 1.500 caracteres por página no IPC;
- 30.000 caracteres no total do viewer.

O parser nativo rejeita página zero, página acima do limite, texto vazio e
payload acima do limite. A superfície nativa só captura enquanto o PDF que
originou o texto continua ativo.

## Segurança

O viewer recebe somente `__neuralia_pdf_page_text(page, text)`, protegido pela
capability da SPEC-0108. Não recebe um dispatcher genérico de ações.

A ação `pdf-page-text` existe no parser global, mas apenas o builder do PDF a
traduz para um evento nativo; outras WebViews a descartam no caminho comum.

## Memória

Cada página vira um `MemoryDocument` de origem `Pdf`, apontando para a URL
original e com título `<arquivo> · página N`. Isso permite que uma busca
semântica recupere a página que contém o conteúdo em vez de guardar apenas que
"um PDF foi aberto".

## Texto selecionável

Páginas renderizadas recebem `pdfjsLib.TextLayer` sobre o canvas. A camada é
libertada com os recursos de render quando a página sai da faixa viva e é
reconstruída após mudança de escala.

## Fora de escopo

Não há OCR nesta fase. PDFs compostos apenas por imagens continuam visíveis, mas
não geram texto pesquisável.

## Gates

- IPC autenticado e limitado para `pdf-page-text`;
- helper do viewer preserva quebras de linha e corta no limite;
- ponte específica, sem `act()` genérico;
- prova de sabotagem obrigatória antes da entrega final.
