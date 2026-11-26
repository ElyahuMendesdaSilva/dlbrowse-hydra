# dlbrowse-hydra

Navegador de fontes JSON do [Hydra](https://github.com/hydralauncher/hydra) para o terminal.
Leve, escrito em Rust com [Ratatui](https://ratatui.rs), feito para computadores fracos.

Ele carrega os JSONs de fontes (os que têm `name` e `downloads`), mostra tudo em uma
interface simples e deixa você pesquisar, filtrar por categoria e abrir os links
direto no navegador ou no cliente de torrent.

## O que ele faz

- Lê um ou vários arquivos JSON (ou pastas inteiras, incluindo subpastas)
- Mostra título, tamanho, data de upload, fornecedor e links de cada item
- Pesquisa por título ou fornecedor
- Agrupa por categoria: fornecedor, mês de upload, tamanho ou host do link
- Ordena por data, título ou tamanho
- Abre os links com o programa padrão do sistema (links `magnet:` abrem no torrent)
- Tem um seletor de arquivos na própria interface e lembra a sua escolha

## Instalação

Precisa do [Rust](https://rustup.rs) instalado.

```bash
git clone https://github.com/ElyahuMendesdaSilva/dlbrowse-hydra.git
cd dlbrowse-hydra
cargo build --release
```

O executável fica em `target/release/dlbrowse-hydra`. Para usar de qualquer pasta:

```bash
mkdir -p ~/.local/bin
cp target/release/dlbrowse-hydra ~/.local/bin/
```

## Como usar

```bash
dlbrowse-hydra
```

Na primeira vez abre o seletor de arquivos. Marque os JSONs ou as pastas que quiser
e aperte `d`. A escolha é salva, e nas próximas vezes o programa abre direto.

Outras formas de abrir:

```bash
dlbrowse-hydra arquivo.json pasta/   # usa esses caminhos e passa a lembrar deles
dlbrowse-hydra --select              # abre o seletor para mudar a seleção
dlbrowse-hydra --clear               # apaga a memória e abre o seletor
dlbrowse-hydra --help
```

A seleção fica salva em `~/.config/dlbrowse-hydra/selection.json`.

## Teclas

| Tecla | Ação |
|---|---|
| `/` | pesquisar (`Enter` confirma, `Esc` limpa) |
| `Tab` / `←` `→` | trocar de painel |
| `↑` `↓` ou `j` `k` | mover |
| `PgUp` `PgDn` / `g` `G` | pular / ir ao início ou fim |
| `Enter` | entrar na lista de links e abrir o link selecionado |
| `o` | abrir o primeiro link do item |
| `c` | trocar o tipo de categoria |
| `s` | trocar a ordenação |
| `f` | abrir o seletor de arquivos |
| `q` | sair |

### No seletor de arquivos

| Tecla | Ação |
|---|---|
| `Enter` / `→` | entrar na pasta ou marcar o arquivo |
| `←` / `Backspace` | subir uma pasta |
| `Espaço` | marcar ou desmarcar |
| `a` | marcar todos os JSON da pasta atual |
| `x` | limpar as marcações |
| `.` | mostrar ou ocultar pastas ocultas |
| `d` | concluir e salvar |
| `Esc` | cancelar |

## Formato dos JSON

```json
{
  "name": "Nome da fonte",
  "downloads": [
    {
      "title": "Título",
      "uris": ["link"],
      "uriGroups": [["link1", "link2"]],
      "uploadDate": "2026-11-20T03:33:52.000Z",
      "fileSize": "5.6 GB"
    }
  ]
}
```

Só `downloads` é necessário. `uriGroups` e os demais campos são opcionais.
Arquivos com JSON inválido são ignorados e listados ao sair do programa.