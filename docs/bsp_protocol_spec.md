# SourceKit-LSP BSP 서버 프로토콜 스펙 (Rust 재구현용)

> **출처**: Phase 3a-0 리서치(2026-07-13). 로컬 `xcode-build-server` 1.3.0 Python 소스 + `swiftlang/sourcekit-lsp` `Contributor Documentation/BSP Extensions.md` 교차검증.
> **상위 문서**: [design_project_recognition_bsp.md](./design_project_recognition_bsp.md) §5·§6·§7
> **용도**: Phase 3a C-네이티브 Rust BSP 서버 구현의 **직접 구현 대상 규약**. JSON 예시는 전부 실제 소스에서 나온 형태.

## 소스 위치
- Python 레퍼런스: `/opt/homebrew/Cellar/xcode-build-server/1.3.0/libexec/` (`/opt/homebrew/bin/xcode-build-server` → `Cellar/.../bin` → `libexec/xcode-build-server`). 핵심: `server.py`(BSP 루프+핸들러), `config/config.py`(buildServer.json), `config/cmd.py`(config 서브커맨드).
- 문서: `swiftlang/sourcekit-lsp` `Contributor Documentation/BSP Extensions.md` (raw: https://raw.githubusercontent.com/swiftlang/sourcekit-lsp/main/Contributor%20Documentation/BSP%20Extensions.md), 베이스 스펙 https://build-server-protocol.github.io/docs/specification

---

## 0. Transport / Framing — LSP 스타일 확정

`server.py:24-32`(쓰기) + `server.py:505-514`(읽기):

- **쓰기**: `Content-Length: {N}\r\n\r\n{json}` (`server.py:28`). `Content-Type` 헤더 안 씀. 헤더-바디 구분 `\r\n\r\n`.
- **읽기**: 한 줄 읽어 `Content-Length:`로 시작 assert → 정수 파싱 → 빈 줄 하나 더 → 정확히 N 바이트.
- `N`은 JSON 길이. Python `json.dumps`는 `ensure_ascii=True`라 char==byte지만, **Rust는 UTF-8 바이트 길이로 Content-Length 계산해야 함**(첫 번째 프레이밍 함정).
- stdout 닫히면 즉시 종료. 모든 메시지 `"jsonrpc":"2.0"`.

---

## 1. `build/initialize` (request) — 가장 중요

Client(SourceKit-LSP) → Server. 핸들러 `server.py:362-408`. 서버가 읽는 params: `params.rootUri`, `params.bspVersion`.

클라이언트 params(`InitializeBuildParams`):
```json
{
  "jsonrpc": "2.0", "id": 0, "method": "build/initialize",
  "params": {
    "displayName": "SourceKitLSP", "version": "1.0.0", "bspVersion": "2.2.0",
    "rootUri": "file:///Users/me/MyApp",
    "capabilities": { "languageIds": ["swift","objective-c","c","cpp","objective-cpp"] },
    "data": {}
  }
}
```

**Result** (`server.py:390-408`) — 인덱스 경로 광고가 핵심:
```json
{
  "jsonrpc": "2.0", "id": 0,
  "result": {
    "displayName": "xcode build server", "version": "1.3.0", "bspVersion": "2.2.0",
    "rootUri": "file:///Users/me/MyApp",
    "capabilities": { "languageIds": ["c","cpp","objective-c","objective-cpp","swift"] },
    "data": {
      "indexDatabasePath": "/Users/me/Library/Caches/xcode-build-server/-Users-me-MyApp/indexDatabasePath-<md5>",
      "indexStorePath": "/Users/me/Library/Developer/Xcode/DerivedData/MyApp-xxxx/Index.noindex/DataStore",
      "sourceKitOptionsProvider": true
    },
    "dataKind": "sourceKit"
  }
}
```

인덱스 광고 규칙(= 공짜 cross-file jump-to-def의 근거, 설계 "B 잔여 가치"):
- 필드명 확정: `indexStorePath`, `indexDatabasePath` — 둘 다 `result.data` 안. `result.dataKind:"sourceKit"`가 이 data를 SourceKit 확장으로 표시(`server.py:401-406`). 문서 `SourceKitInitializeBuildResponseData`와 일치.
- `indexStorePath` = Xcode DerivedData `<build_root>/Index.noindex/DataStore`(`server.py:107-114`). 이게 있어야 Xcode가 쌓아둔 인덱스로 전역 정의점프 즉시 동작.
- `indexDatabasePath` = SourceKit-LSP 전용 DB(indexStorePath md5로 유니크, `server.py:386-388`). 없으면 캐시 하위로 폴백.
- `sourceKitOptionsProvider:true`로 `sourceKitOptions` 구현 광고.

퀵:
1. `capabilities`는 베이스 BSP의 `BuildServerCapabilities`가 **아니라** 비표준 `{"languageIds":[...]}` 형태(`server.py:398-400`). SourceKit-LSP 수용.
2. `prepareProvider`/`outputPathsProvider` 광고 안 함 → `buildTarget/prepare`·source `outputPath` 미지원이어도 동작.
3. `bspVersion`이 "새 버전 모드" 스위치: 클라 `bspVersion >= "2.2.0"`이면 `new_version=True`(`server.py:372`) → 무효화를 `buildTarget/didChange`로(§6).

---

## 2. `build/initialized` (notification)

Client → Server. `server.py:410-411`. params `{}`. 응답 없음(감시 스레드 시작).
```json
{ "jsonrpc": "2.0", "method": "build/initialized", "params": {} }
```

---

## 3. `workspace/buildTargets` (request)

Client → Server. `server.py:420-447`. params 무시. xcode-build-server는 **단일 더미 타깃**만 반환:
```json
{
  "jsonrpc": "2.0", "id": 1,
  "result": { "targets": [
    { "id": { "uri": "dummy://dummy" }, "displayName": "BuildServer",
      "tags": ["test"], "capabilities": {},
      "languageIds": ["c","cpp","objective-c","objective-cpp","swift"], "dependencies": [] }
  ] }
}
```
`BuildTarget` 필드: `id.uri`(필수), `displayName?`, `baseDirectory?`, `tags[]`, `languageIds[]`, `dependencies[]`, `capabilities{canCompile?,canTest?,canRun?,canDebug?}`. jump-to-def는 인덱스 스토어가 해결하므로 더미 타깃 하나로 충분.

---

## 4. `buildTarget/sources` (request)

Client → Server. `server.py:449-466`. 파일 열거 없이 **루트 디렉터리 하나**:
```json
// params
{ "jsonrpc":"2.0","id":2,"method":"buildTarget/sources",
  "params": { "targets": [ { "uri": "dummy://dummy" } ] } }
// result
{ "jsonrpc":"2.0","id":2,
  "result": { "items": [
    { "target": { "uri": "dummy://dummy" },
      "sources": [ { "uri": "file:///Users/me/MyApp", "kind": 2, "generated": false } ] }
  ] } }
```
- `SourceItem.kind`: **정수** — `1`=file, `2`=directory. `generated`: bool.
- `roots`/`dataKind`/`outputPath` 미사용. `params.targets`에 `dummy://dummy` 아닌 건 결과에서 제외.

---

## 5. `textDocument/sourceKitOptions` (request) — 핵심 질의

Client → Server. `server.py:480-486`, 로직 `server.py:146-162`.

**메서드명 casing 함정(최대 재구현 리스크)**:
- 1.3.0은 프리픽스 없는 `textDocument/sourceKitOptions`만 등록 → 현행 이 머신 SourceKit-LSP와 정상 동작(실증).
- 최신 문서는 정식명을 `sourcekit/textDocument/sourceKitOptions`로 바꾸고 옛 이름 하위호환 수용.
- **Rust 권고: `textDocument/sourceKitOptions`와 `sourcekit/textDocument/sourceKitOptions` 둘 다 같은 핸들러로 라우팅.** (`buildTarget/prepare`↔`sourcekit/…`, `workspace/waitForBuildSystemUpdates`↔`sourcekit/…`도 양쪽 수용.)

```json
// params (서버가 실제로 읽는 건 textDocument.uri)
{ "jsonrpc":"2.0","id":3,"method":"textDocument/sourceKitOptions",
  "params": { "textDocument": { "uri": "file:///.../A.swift" },
              "target": { "uri": "dummy://dummy" }, "language": "swift" } }
// result (성공)
{ "jsonrpc":"2.0","id":3,
  "result": {
    "compilerArguments": ["-module-name","MyApp","-sdk","/.../MacOSX.sdk",
      "-I","/.../Debug","-working-directory","/Users/me/MyApp","/Users/me/MyApp/Sources/A.swift"],
    "workingDirectory": "/Users/me/MyApp" } }
```
- `compilerArguments`: `[String]` — 컴파일러 인자 전체(파일 경로 포함). **← C 합성 결과를 여기 넣는다.**
- `workingDirectory`: optional String.
- **실패/옵션없음 = 에러 아님, `result: null`** (`server.py:162,485`). JSON-RPC error 금지.
```json
{ "jsonrpc": "2.0", "id": 3, "result": null }
```

---

## 6. 캐시 무효화 / 변경 통지 (설계 §6.6)

협상된 `bspVersion`에 따라 두 방식(`server.py:340-352`):

**(A) 새 버전(≥2.2.0) — `buildTarget/didChange`** — 현행 SourceKit-LSP가 쓰는 경로:
```json
{ "jsonrpc":"2.0","method":"buildTarget/didChange","params": { "changes": null } }
```
`changes:null` = "모든 타깃 변경" → SourceKit-LSP가 sourceKitOptions 재질의. 무효화 트리거(pbxproj/xcconfig 변경) 시 이걸 보낸다. `buildTargetChangedProvider` 광고 없이 수용됨.

**(B) 옛 버전 — `build/sourceKitOptionsChanged`** (레거시, `server.py:187-199`): `textDocument/registerForChanges` 흐름과 짝. 2.2.0에선 (A)만 쓰이므로 Rust는 (A)만 구현해도 충분.

기타: `workspace/waitForBuildSystemUpdates`(`server.py:413-418`)는 `{}` 반환 no-op(`sourcekit/` 프리픽스도 수용).

---

## 7. `build/shutdown` + `build/exit`

```json
// build/shutdown (request) → result null
{ "jsonrpc":"2.0","id":9,"method":"build/shutdown","params":{} }
{ "jsonrpc":"2.0","id":9,"result":null }
// build/exit (notification) → 프로세스 즉시 종료, 응답 없음
{ "jsonrpc":"2.0","method":"build/exit","params":{} }
```

---

## 8. `buildServer.json` 필드 (config.py 기준)

SourceKit-LSP가 읽는 발견/구동 계약:
```json
{ "name":"xcode build server", "version":"1.3.0", "bspVersion":"2.2.0",
  "languages":["c","cpp","objective-c","objective-cpp","swift"],
  "argv":["/opt/homebrew/bin/xcode-build-server"] }
```
- **의미 있는 필드**: `argv`(서버 실행 커맨드 — SourceKit-LSP가 이걸로 스폰; **가장 중요**), `bspVersion`, `languages`, `name`, `version`. **우리 Rust 바이너리 절대경로+인자를 `argv`에 넣는다.**
- `kind`/`workspace`/`scheme`/`build_root`/`indexStorePath` 등은 xcode-build-server 자체 설정이라 SourceKit-LSP는 무시. 우리 서버는 자기 규약대로 재정의 가능.

---

## 9. Rust 재구현 함정 정리

1. **Content-Length = UTF-8 바이트 길이**. 헤더 `\r\n\r\n`, `Content-Type` 없음.
2. **메서드명 프리픽스 이중화**: `textDocument/sourceKitOptions` + `sourcekit/textDocument/sourceKitOptions`(및 prepare, waitForBuildSystemUpdates) 둘 다 수용.
3. **sourceKitOptions 실패는 error 아님 `result: null`**. JSON-RPC error 금지.
4. **초기화 순서 강제**: `build/initialize` 전 상태 없음. 그 전 요청은 id 있으면 에러(code 123), notification이면 무시.
5. **미지원 메서드**: id 있으면 `{"error":{"code":123,"message":"unhandled method X"}}`, notification은 조용히 무시. `$/cancelRequest`도 무시.
6. **인덱스 광고 위치 고정**: `result.data.indexStorePath`/`indexDatabasePath` + `result.dataKind:"sourceKit"`. `indexStorePath`는 반드시 DerivedData `Index.noindex/DataStore`.
7. **capabilities는 비표준 `{languageIds:[...]}`**로 충분.
8. **무효화 = `buildTarget/didChange` + `changes:null`** 한 방(2.2.0). 광고 불필요.
9. `buildTarget/sources`=루트 dir 1개(`kind:2`), `workspace/buildTargets`=`dummy://dummy` 단일 타깃으로 최소 구현 가능(정의점프는 인덱스 스토어 담당).
10. `SourceItem.kind`는 **정수**(1/2).

참조: `server.py`, `config/config.py`, `config/cmd.py` (경로는 위 §소스 위치).
