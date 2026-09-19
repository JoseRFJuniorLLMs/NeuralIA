# Proveniência do PDF.js vendorizado

`pdf.mjs` e `pdf.worker.mjs` são do PDF.js, copiados sem alterações. Os outros
ficheiros desta pasta (`viewer.html`, `viewer.mjs`) são do NeuralIA e não têm
upstream.

| Campo | Valor |
|---|---|
| Projeto | Mozilla PDF.js |
| Versão | **6.3.289** |
| Origem | https://github.com/mozilla/pdf.js — release `v6.3.289`, build `pdfjs-6.3.289-dist.zip` |
| Licença | Apache-2.0 (`LICENSE`, nesta pasta) |
| Alterações locais | nenhuma |

## Ficheiros

| Ficheiro | SHA-256 |
|---|---|
| `pdf.mjs` | `d90d57eceac606b8d0f70872aeb632571bb6d2322540a1551a70d516cd0b29cb` |
| `pdf.worker.mjs` | `2c36ad19110a73873fbdcc30124a445e1e681a5bbe6463546d56696f1124ced5` |
| `LICENSE` | `eb3d7b5485466acbd81f2b496f595ab637d2792e268206b27d99e793bdb67549` |

## Porque é que isto está preso por um teste

O `release.yml` acrescenta ao SBOM (CycloneDX) um componente `pdf.js` com a
versão escrita à mão no próprio workflow. Sem nada a ligar as duas pontas, uma
atualização do `pdf.mjs` deixava o SBOM publicado a declarar uma versão que já
não é a que embarca — uma afirmação falsa num artefacto de release, que é o
sítio onde ela custa mais.

`crates/neural-core/tests/vendored_provenance.rs` compara três coisas que têm de
dizer o mesmo número: a `const version` dentro do `pdf.mjs`, a tabela acima, e a
versão escrita no `release.yml`. Atualizar o PDF.js implica atualizar este
ficheiro — é esse o objetivo.
