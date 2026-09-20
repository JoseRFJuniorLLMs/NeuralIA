# SPEC-0110 — PDF Reader textual

**Status:** Proposta — implementação em revisão.

## Objetivo

O visualizador PDF do NeuralIA deixa de ser apenas uma imagem rolável. O texto de páginas renderizadas deve ser selecionável e copiável, e uma extração textual limitada deve entrar na memória sem comprometer o orçamento de documentos grandes.

## Caminho que embarca

- PDF.js continua responsável por parsing/render e pelos assets offline do #91.
- Cada página visível recebe a `TextLayer` oficial do PDF.js sobre o canvas.
- Após o documento abrir, o viewer extrai texto em segundo plano com `getTextContent()`.
- O texto viaja para o nativo pelo canal IPC autenticado da SPEC-0108, em chunks limitados.
- O nativo só publica um `MemoryDocument` depois do marcador final; navegação para outro documento invalida a geração anterior.

## Limites

A extração para memória é deliberadamente diferente da visualização. O utilizador pode continuar a ver o PDF inteiro, mas a captura automática para memória é limitada a:

- 256 páginas;
- 512 KiB de caracteres acumulados;
- 1000 code points por mensagem do viewer, com limite nativo de 4096 bytes no campo IPC;
- corpo IPC total continua limitado pelos 8 KiB da SPEC-0108.

Ao atingir qualquer limite, a memória recebe uma marca explícita de extração truncada. Um PDF sem texto extraível continua abrindo normalmente e gera apenas um registo mínimo, em vez de fingir que OCR aconteceu.

## Pesquisa e sessão

A captura usa `MemorySourceKind::Pdf`. Em sessão de pesquisa, a mesma fonte é associada à sessão com o texto extraído; fora de sessão ela entra apenas na memória semântica normal.

## Segurança

- O bridge é instalado somente no visualizador PDF interno.
- A mensagem usa a capability aleatória do canal IPC existente.
- Páginas remotas não recebem o bridge.
- Chunks fora dos limites, páginas zero/absurdas e payloads com campos extras são rejeitados no parser nativo.
- Eventos de uma geração antiga são descartados antes de tocar na memória.

## Gate de aceitação

O gate deve provar:

1. normalização e chunking Unicode;
2. limite de páginas e caracteres;
3. parser IPC fail-closed para chunks inválidos;
4. mapeamento do `IpcAction::PdfText` para a geração correta do produto;
5. acumulador nativo só finaliza no marcador `done` e nunca ultrapassa o orçamento;
6. viewer PDF.js e os cinco casos auxiliares do #91 continuam verdes.

A prova de sabotagem remove o limite ou desliga o mapeamento `PdfText → UserEvent` e deve deixar o gate vermelho; a sabotagem não entra no commit final.
