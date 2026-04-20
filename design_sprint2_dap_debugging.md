# Sprint 2 설계 문서: DAP 디버깅 구현

> **프로젝트**: Xcode Tools for Zed (Alternative-Xcode)
> **작성일**: 2026-04-12
> **대상**: `src/lib.rs`, `debug_adapter_schemas/xcode-debug.json`
> **참조**: `docs/architecture.md`, `docs/v0.1_spec.md`

---

## 1. 기존 계획에 대한 비판적 검토

### 🔴 치명적 문제 (반드시 수정)

#### 1-1. `env` 포맷 불일치 — 심각도: **CRITICAL**

| 항목 | 현재 설계 (스키마 + architecture.md) | lldb-dap 실제 기대값 |
|------|--------------------------------------|---------------------|
| **타입** | JSON Object (`{"FOO": "1", "BAR": "2"}`) | String Array (`["FOO=1", "BAR=2"]`) |
| **스키마 정의** | `"type": "object", "additionalProperties": {"type": "string"}` | `"type": "array", "items": {"type": "string"}` |
| **Rust 타입** | `HashMap<String, String>` | `Vec<String>` |

**영향**: 사용자가 `debug.json`에 환경변수를 설정해도 lldb-dap에 전달되지 않음. 디버깅 세션에서 환경변수가 완전히 무시됨.

**근거**: [lldb-dap 공식 문서](https://lldb.llvm.org/use/lldbdap.html) — *"The format of each environment variable string is `VAR=VALUE` for environment variables with values or just `VAR` for environment variables with no values"*

#### 1-2. `processId` vs `pid` — 심각도: **CRITICAL**

| 항목 | 현재 설계 | lldb-dap 실제 기대값 |
|------|-----------|---------------------|
| **필드명** | `processId` | `pid` |
| **스키마** | `"processId": {"type": "integer"}` | `"pid": {"type": "integer"}` |

**영향**: Attach 모드가 완전히 실패. lldb-dap가 `pid` 필드를 찾지 못해 대상 프로세스에 연결 불가.

**근거**: [llvm/llvm-project lldb-dap README](https://github.com/llvm/llvm-project/blob/main/lldb/tools/lldb-dap/README.md) — *"The process id of the process you wish to attach to"* 필드명이 `pid`.

#### 1-3. Raw config 패스스루 (변환 없는 직접 전달) — 심각도: **CRITICAL**

`architecture.md`의 `get_dap_binary()` 구현:
```rust
request_args: zed::StartDebuggingRequestArguments {
    configuration: config.config,  // ← 사용자 JSON을 변환 없이 그대로 전달
    request,
},
```

**문제**: `configuration` 필드는 Zed가 lldb-dap에 DAP launch/attach 요청 인자로 직접 전달하는 JSON. 사용자 스키마(processId, env as object)와 lldb-dap 기대 포맷(pid, env as array)이 다르므로, **중간 변환 레이어가 반드시 필요**.

현재 설계는 이 변환을 완전히 누락하고 있음.

---

### 🟡 중요 누락 사항

#### 1-4. lldb-dap 전용 유용 필드 미포함 — 심각도: MEDIUM

lldb-dap는 다음 필드를 추가로 지원하며, Xcode 프로젝트 디버깅에 실질적으로 유용:

| 필드 | 용도 | v0.1 필요도 |
|------|------|-------------|
| `initCommands` | lldb-dap 초기화 시 실행할 LLDB 명령어 | 선택 (v0.2 추천) |
| `preRunCommands` | target 생성 후, launch 전 실행할 LLDB 명령어 | 선택 (v0.2 추천) |
| `waitFor` | (attach) 아직 실행되지 않은 프로세스를 대기하며 attach | **권장** — iOS 앱 디버깅 시 필수적 |
| `sourceMap` | 소스 경로 매핑 | 선택 (v0.2) |

특히 `waitFor`는 시뮬레이터 앱 디버깅 워크플로우에서 중요: 앱을 simctl로 launch하기 전에 디버거를 attach 대기 상태로 둘 수 있음.

#### 1-5. `DebugScenario.build` 필드 미활용 — 심각도: MEDIUM

```wit
record debug-scenario {
    // ...
    build: option<build-task-definition>,  // ← 디버그 전 빌드 단계
    // ...
}
```

Xcode 프로젝트는 디버깅 전 빌드가 거의 항상 필요. `dap_config_to_scenario()`에서 이 필드를 활용하면 "Build & Debug" 워크플로우를 자동화할 수 있음. v0.1에서는 `None`으로 두되, 향후 확장을 위해 설계에 명시해야 함.

#### 1-6. 사용자 UX: 두 가지 스키마 개념 혼동 우려 — 심각도: LOW

현재 설계에서 사용자가 `.zed/debug.json`에 작성하는 스키마와 lldb-dap가 받는 스키마가 다름. 이는 의도적인 추상화이지만, 문서화가 필요.

- **사용자 스키마** (`xcode-debug.json`): 사용자 친화적 (camelCase, env as object)
- **lldb-dap 스키마**: DAP 프로토콜 준수 (pid, env as string array)
- **변환 책임**: 우리 extension의 `get_dap_binary()`

---

### 🟢 계획 내 중복/불필요 항목

#### 1-7. Step 1 "DAP 스키마 작성"은 이미 완료

`debug_adapter_schemas/xcode-debug.json` (44줄)이 이미 존재. 내용 **수정**은 필요하지만 신규 작성이 아님.

#### 1-8. Step 2 의존성은 이미 추가됨

`Cargo.toml`에 `serde`, `serde_json`이 이미 있음:
```toml
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

`XcodeDebugConfig` 구조체 정의만 추가하면 됨.

---

## 2. 수정된 설계

### 2.1 스키마 수정 (`debug_adapter_schemas/xcode-debug.json`)

사용자 경험과 lldb-dap 호환성을 동시에 달성하기 위한 **이중 스키마 전략**:

- **사용자 스키마** (xcode-debug.json): 직관적인 형식 유지 → env는 object, processId 사용
- **내부 변환**: extension이 get_dap_binary()에서 lldb-dap 호환 형식으로 변환

다만, v0.1의 단순성을 위해 **lldb-dap 네이티브 형식을 스키마로 직접 채택**하는 것이 더 적합:

```jsonc
{
  "properties": {
    "request": { "type": "string", "enum": ["launch", "attach"] },
    "program": { "type": "string" },
    "cwd": { "type": "string", "default": "${ZED_WORKTREE_ROOT}" },
    "args": { "type": "array", "items": {"type": "string"}, "default": [] },
    "env": {
      "type": "array",                          // ← object → array 변경
      "items": { "type": "string" },             // "KEY=VALUE" 형식
      "description": "Environment variables in KEY=VALUE format",
      "default": []
    },
    "pid": {                                     // ← processId → pid 변경
      "type": "integer",
      "description": "Process ID to attach to (attach mode only)"
    },
    "stopOnEntry": { "type": "boolean", "default": false },
    "waitFor": {                                 // ← 신규 추가
      "type": "boolean",
      "description": "Wait for process to launch before attaching (attach mode only)",
      "default": false
    }
  }
}
```

**이유**: 사용자가 작성하는 JSON이 lldb-dap에 (변환 없이 or 최소 변환으로) 직접 전달되면 디버깅이 쉬워짐. 중간 변환 레이어에서 버그가 발생할 여지를 줄임.

### 2.2 Rust 타입 정의 (`XcodeDebugConfig`)

```rust
use serde::{Deserialize, Serialize};

const ADAPTER_NAME: &str = "xcode-debug";

/// 사용자의 .zed/debug.json config를 파싱하는 구조체.
/// lldb-dap의 launch/attach request arguments 형식과 1:1 대응.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct XcodeDebugConfig {
    request: String,                              // "launch" | "attach"
    #[serde(default)] program: Option<String>,    // 실행 파일 경로
    #[serde(default)] cwd: Option<String>,        // 작업 디렉토리
    #[serde(default)] args: Vec<String>,          // 실행 인자
    #[serde(default)] env: Vec<String>,           // ← Vec<String> ("KEY=VALUE")
    #[serde(default)] pid: Option<u32>,           // ← pid (processId 아님)
    #[serde(default)] stop_on_entry: Option<bool>,
    #[serde(default)] wait_for: Option<bool>,     // ← 신규: attach 대기 모드
}
```

**주의**: `#[serde(rename_all = "camelCase")]`에 의해:
- `stop_on_entry` → JSON `stopOnEntry` ✓
- `wait_for` → JSON `waitFor` ✓
- `pid` → JSON `pid` ✓ (이미 소문자 단일 단어)

### 2.3 lldb-dap용 설정 변환 함수

`configuration` 필드에 전달할 JSON을 생성하는 전용 함수:

```rust
/// XcodeDebugConfig를 lldb-dap가 이해하는 JSON으로 직렬화.
/// 스키마가 lldb-dap 네이티브 형식이므로 사실상 serde_json::to_string과 동일하지만,
/// request 필드 제거 등 불필요한 필드를 정리하는 역할.
fn build_dap_configuration(config: &XcodeDebugConfig) -> Result<String, String> {
    // lldb-dap는 configuration에서 request 필드를 기대하지 않음
    // (request는 StartDebuggingRequestArguments.request로 별도 전달됨)
    // 따라서 request를 제외한 나머지만 직렬화
    let mut value = serde_json::to_value(config)
        .map_err(|e| format!("Serialize error: {e}"))?;
    if let Some(obj) = value.as_object_mut() {
        obj.remove("request");
    }
    serde_json::to_string(&value)
        .map_err(|e| format!("Serialize error: {e}"))
}
```

### 2.4 3개 Extension Trait 메서드 설계

#### `get_dap_binary()` — 디버거 바이너리 + 설정 반환

```
호출 시점: Zed가 디버그 세션을 시작할 때
입력: DebugTaskDefinition (사용자 debug.json의 raw JSON)
출력: DebugAdapterBinary (lldb-dap 경로 + DAP 요청 인자)
```

핵심 흐름:
1. adapter_name 검증 (`"xcode-debug"`)
2. `config.config` (JSON string) → `XcodeDebugConfig` 파싱
3. request 타입 판별 (launch/attach)
4. `resolve_dap_binary()`로 lldb-dap 경로 탐색
5. `build_dap_configuration()`으로 lldb-dap용 JSON 생성
6. `DebugAdapterBinary` 구성하여 반환

**변경점 vs architecture.md**:
- `configuration: config.config` (raw 전달) → `configuration: build_dap_configuration(&parsed)?` (변환 후 전달)
- `envs: vec![]` → 필요시 lldb-dap 프로세스 자체의 환경변수 설정 가능 (v0.1은 빈 벡터 유지)

#### `dap_request_kind()` — launch/attach 판별

```
호출 시점: Zed가 디버그 설정의 종류를 판별할 때
입력: serde_json::Value (debug.json config의 JSON)
출력: StartDebuggingRequestArgumentsRequest (Launch | Attach)
```

이 메서드는 단순 판별만 수행. architecture.md 설계와 동일하게 유지.

#### `dap_config_to_scenario()` — DebugConfig → DebugScenario 변환

```
호출 시점: Zed의 "New Debug Session" UI에서 설정을 생성할 때
입력: DebugConfig { label, adapter, request: DebugRequest, stop_on_entry }
출력: DebugScenario { label, adapter, build, config, tcp_connection }
```

핵심 흐름:
1. `DebugRequest` variant에 따라 `XcodeDebugConfig` 생성
2. **env 변환**: `DebugRequest::Launch.envs`는 `Vec<(String, String)>` → `Vec<String>` ("KEY=VALUE")로 변환
3. `XcodeDebugConfig`를 JSON string으로 직렬화 → `DebugScenario.config`에 설정
4. `build: None` (v0.1), `tcp_connection: None`

**핵심 변환 코드**:
```rust
// launch.envs: Vec<(String, String)> → Vec<String> ("KEY=VALUE")
let env: Vec<String> = launch.envs.iter()
    .map(|(k, v)| if v.is_empty() { k.clone() } else { format!("{k}={v}") })
    .collect();
```

### 2.5 DAP 바이너리 탐색 (`resolve_dap_binary`)

architecture.md의 3단계 fallback chain 유지. 변경 없음:

```
1. user_provided_debug_adapter_path (있으면 사용)
2. worktree.which("xcrun") → ("xcrun", ["lldb-dap"])
3. worktree.which("lldb-dap") → (path, [])
4. Error
```

### 2.6 데이터 흐름 요약

```
[사용자 작성]                    [Extension 처리]                [Zed → lldb-dap]
.zed/debug.json          →   get_dap_binary()            →   DAP Launch/Attach Request
{                              1. JSON 파싱                    {
  "request": "launch",         2. XcodeDebugConfig 생성          "program": "/path/to/app",
  "program": "/path",          3. lldb-dap 경로 탐색              "args": [],
  "env": ["FOO=1"],            4. build_dap_configuration()       "env": ["FOO=1"],
  "stopOnEntry": true          5. DebugAdapterBinary 반환         "stopOnEntry": true
}                                                              }

[Zed New Session UI]         [Extension 처리]                [결과]
DebugConfig              →   dap_config_to_scenario()    →   DebugScenario
{                              1. DebugRequest 분기              {
  label, adapter,              2. env 형식 변환                    label, adapter,
  request: Launch{...},        3. XcodeDebugConfig 생성            config: "...(JSON)",
  stop_on_entry                4. JSON 직렬화                      build: None
}                                                              }
```

---

## 3. 수정된 구현 계획

### Step 1: 스키마 수정 (debug_adapter_schemas/xcode-debug.json)

**작업**: 기존 파일 수정 (신규 작성 아님)

| 변경 | Before | After |
|------|--------|-------|
| `env` 타입 | `"type": "object"` | `"type": "array", "items": {"type": "string"}` |
| `processId` | `"processId"` | `"pid"` |
| `waitFor` | 없음 | `"type": "boolean", "default": false` 추가 |
| 조건부 required | `"program"` (launch만) | 유지 |

### Step 2: Rust 구현 (src/lib.rs)

**작업**: 현재 11줄 스켈레톤 → 전체 DAP 구현

구현 순서:
1. `XcodeDebugConfig` 구조체 정의 (env: `Vec<String>`, pid: `Option<u32>`)
2. `build_dap_configuration()` 함수 (config → lldb-dap JSON)
3. `resolve_dap_binary()` 함수 (3단계 fallback)
4. `get_dap_binary()` trait 메서드
5. `dap_request_kind()` trait 메서드
6. `dap_config_to_scenario()` trait 메서드

### Step 3: 빌드 검증

```bash
cargo build --target wasm32-wasip2
```

WASM 컴파일 성공 확인. 타입 불일치나 API 시그니처 오류를 이 단계에서 포착.

### Step 4: E2E 검증 준비

`.zed/debug.json` 템플릿 예시:

```json
{
  "label": "Debug My App",
  "adapter": "xcode-debug",
  "config": {
    "request": "launch",
    "program": "${ZED_WORKTREE_ROOT}/build/Debug/MyApp",
    "args": [],
    "env": ["DYLD_PRINT_LIBRARIES=1"],
    "stopOnEntry": false
  }
}
```

검증 항목:
- [ ] lldb-dap 경로가 정상 탐색되는가 (xcrun lldb-dap)
- [ ] 브레이크포인트에서 정상 중단되는가
- [ ] 변수 조회 (Variables 패널)가 작동하는가
- [ ] Step Over/Into/Out이 작동하는가
- [ ] 환경변수가 디버기 프로세스에 전달되는가

---

## 4. 리스크 및 대응

| ID | 리스크 | 심각도 | 대응 |
|----|--------|--------|------|
| R1 | `zed_extension_api` 0.7.0의 실제 타입이 WIT와 다를 수 있음 | HIGH | Step 3 빌드에서 즉시 발견. 컴파일 에러 메시지로 실제 타입 확인 가능 |
| R2 | `worktree.which("xcrun")`이 WASM 샌드박스에서 실패 | HIGH | fallback chain의 3단계가 이를 커버. 실패 시 에러 메시지로 사용자에게 수동 경로 설정 안내 |
| R3 | lldb-dap가 `configuration` JSON의 알 수 없는 필드를 거부 | MEDIUM | `build_dap_configuration()`에서 lldb-dap에 불필요한 필드(request 등)를 제거 |
| R4 | `serde(rename_all = "camelCase")`와 lldb-dap 필드명 불일치 | LOW | `pid`, `cwd`, `env`, `args`는 모두 소문자 단일 단어 → camelCase 변환 없이 그대로 유지됨 |

---

## 5. 향후 확장 (v0.2 참고)

v0.1에서는 구현하지 않지만, 설계 시 고려:

- `initCommands` / `preRunCommands`: LLDB 커스텀 명령어 지원
- `DebugScenario.build`: 빌드 후 디버그 자동 워크플로우
- `sourceMap`: 경로 매핑 (프레임워크 디버깅)
- Custom DAP Wrapper: lldb-dap를 래핑하여 추가 기능 제공

---

## 참조

- [lldb-dap 공식 문서 — Getting started](https://lldb.llvm.org/use/lldbdap.html)
- [llvm/llvm-project lldb-dap README](https://github.com/llvm/llvm-project/blob/main/lldb/tools/lldb-dap/README.md)
- [zed_extension_api 0.7.0 WIT 정의](~/.cargo/registry/src/index.crates.io-*/zed_extension_api-0.7.0/wit/since_v0.6.0/dap.wit)
- `docs/architecture.md` §3 — WASM Extension 설계
- `docs/v0.1_spec.md` §S2 — Sprint 2 요구사항
