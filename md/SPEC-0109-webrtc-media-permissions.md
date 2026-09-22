# SPEC-0109 — WebRTC com consentimento nativo

**Status:** Implementada nesta branch após gate verde.

## Objetivo

Permitir chamadas WebRTC reais nas superfícies web visíveis do NeuralIA sem
transformar a política de permissões em um `Allow` genérico.

O WebRTC (ICE, DTLS/SRTP, `RTCPeerConnection`, data channels) continua a ser
fornecido pelo WebView2. O NeuralIA controla apenas a decisão de host sobre
pedidos de captura local.

## Política

Em Full Web, comparador e Split visíveis:

- microfone → `PermissionResponse::Default`;
- câmera → `PermissionResponse::Default`;
- captura de tela/janela → `PermissionResponse::Default`.

No WRY 0.57, `Default` continua o fluxo nativo de permissões do WebView2 no
Windows. O NeuralIA nunca responde `Allow` para essas capacidades; a decisão
permanece com o usuário/runtime.

Continuam em `Deny`:

- geolocalização;
- notificações;
- leitura do clipboard;
- sensores;
- fontes locais;
- filesystem e demais permissões fora de captura.

Quando a mesma construção de Full Web é usada pelo agente
(`agent_enabled=true`), câmera, microfone e captura de tela também ficam em
`Deny`. Automação não pode provocar um pedido de captura como efeito colateral.

Reader, PDF e monitor Gmail mantêm seus handlers fail-closed existentes.

## Gate

Os testes no caminho que embarca exigem que:

1. câmera, microfone e display capture numa superfície visível retornem
   `Default` e nunca `Allow`;
2. permissões fora do escopo retornem `Deny`;
3. as três capturas retornem `Deny` quando a superfície não é de interação
   humana.

A prova de sabotagem troca temporariamente `Camera` para `Deny`; o primeiro
teste deve ficar vermelho. A sabotagem não entra na branch final.
