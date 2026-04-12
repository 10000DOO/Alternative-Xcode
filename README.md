# Xcode Tools for Zed

Zed 에디터에서 Xcode 없이 iOS/macOS 앱을 **빌드, 실행, 테스트, 디버그**할 수 있게 해주는 extension.

기존 Swift/Objective-C extension과 충돌 없이 공존하며, Xcode 워크플로우 대체에만 집중합니다.

## 주요 기능

| 기능 | 설명 |
|------|------|
| **Build** | `xcodebuild build` — scheme 자동 탐색 + 번호 선택 |
| **Run (macOS)** | 빌드 후 macOS 앱 자동 실행 |
| **Run (Simulator)** | 빌드 후 iOS Simulator에 앱 설치 + 실행 |
| **Test** | `xcodebuild test` — XCTest, Swift Testing 지원 |
| **Clean** | `xcodebuild clean` — 빌드 산출물 정리 |
| **Debug** | DAP 기반 브레이크포인트 디버깅 (lldb-dap) |

## 요구 사항

- macOS
- [Zed](https://zed.dev) (최신 버전 권장)
- Xcode + Command Line Tools (`xcode-select --install`)
- (권장) [Swift extension](https://zed.dev/extensions) 설치
- (선택) [xcbeautify](https://github.com/cpisciotta/xcbeautify) — 설치하면 빌드 출력이 깔끔해집니다

---

## 설치 방법

### 1단계: Extension 설치

Zed에서 Dev Extension으로 설치합니다.

1. 이 저장소를 클론합니다:
   ```bash
   git clone https://github.com/10000DOO/Alternative-Xcode.git
   ```

2. Zed를 열고 `Cmd+Shift+P` → **"zed: install dev extension"** 입력

3. 클론한 `Alternative-Xcode` 폴더를 선택합니다

4. Extension이 로드되면 완료 (하단 상태바에서 확인 가능)

### 2단계: Task 설정 (1회만 실행)

터미널에서 setup 스크립트를 **한 번만** 실행합니다:

```bash
cd Alternative-Xcode
bash scripts/setup.sh
```

실행하면 이런 화면이 나옵니다:

```
  ╔══════════════════════════════════════╗
  ║    Xcode Tools for Zed — Setup       ║
  ╚══════════════════════════════════════╝

[INFO] Step 1/3: helpers.sh 설치
[OK] Installed: /Users/you/.config/zed/xcode-tools/helpers.sh
[INFO] Step 2/3: 기존 tasks.json 백업
[OK] Backup: /Users/you/.config/zed/tasks.json.backup.20260412_180000
[INFO] Step 3/3: tasks.json 구성
[OK] tasks.json 구성 완료

Setup complete!
```

**이게 뭘 하는 건가요?**
- `helpers.sh`를 `~/.config/zed/xcode-tools/`에 복사합니다
- 기존 `tasks.json`이 있으면 백업한 뒤 새로 구성합니다
- Zed의 Task 목록에 Xcode 관련 명령이 자동으로 추가됩니다

> 기존에 xbuild 기반 task를 쓰고 있었다면, setup이 자동으로 교체합니다.
> 원본은 `tasks.json.backup.날짜` 파일로 보관됩니다.

---

## 사용 방법

### 빌드

1. Zed에서 Xcode 프로젝트 폴더를 엽니다
2. `Cmd+Shift+P` → **"task: spawn"** (또는 단축키)
3. **"Xcode: Build (Debug)"** 선택

```
=== Available Schemes (MyApp.xcodeproj) ===
  1) MyApp
  2) MyAppTests
  3) MyApp-Release

Select scheme number (or 'all'): 1

=== Building: MyApp (Debug) ===
Compiling ViewController.swift
Compiling AppDelegate.swift
Linking MyApp
[OK] SUCCEEDED: MyApp (Debug)
```

- scheme이 1개뿐이면 자동으로 선택됩니다 (번호 입력 불필요)
- `xcbeautify`가 설치되어 있으면 출력이 자동으로 포맷됩니다

### 실행 (macOS)

**"Xcode: Run (macOS)"** 선택 → 빌드 후 macOS 앱이 자동으로 실행됩니다.

### 실행 (iOS Simulator)

**"Xcode: Run (Simulator)"** 선택 → 빌드 후 iPhone 시뮬레이터가 부팅되고 앱이 설치/실행됩니다.

- 기본 시뮬레이터: **iPhone 17 Pro**
- 변경하려면: `export XCODE_TOOLS_SIMULATOR="iPhone 15 Pro"` (셸 설정에 추가)

### 테스트

**"Xcode: Test"** 선택 → `xcodebuild test` 실행, 결과가 터미널에 표시됩니다.

### 클린

**"Xcode: Clean"** 선택 → 빌드 산출물을 삭제합니다.

### Scheme 목록 보기

**"Xcode: List Schemes"** 선택 → 프로젝트의 모든 scheme을 표시합니다.

---

## 사용 가능한 Task 전체 목록

| Task 이름 | 설명 |
|-----------|------|
| Xcode: Build (Debug) | Debug 빌드 |
| Xcode: Build (Release) | Release 빌드 |
| Xcode: Build All (Debug) | 모든 scheme Debug 빌드 |
| Xcode: Clean Build (Debug) | Clean 후 Debug 빌드 |
| Xcode: Clean Build (Release) | Clean 후 Release 빌드 |
| Xcode: Run (macOS) | 빌드 + macOS 앱 실행 |
| Xcode: Run (Simulator) | 빌드 + iOS Simulator 앱 실행 |
| Xcode: Test | 테스트 실행 |
| Xcode: Clean | 빌드 산출물 삭제 |
| Xcode: List Schemes | Scheme 목록 표시 |

---

## 설정

### Simulator 이름 변경

기본값은 `iPhone 17 Pro`입니다. 변경하려면 셸 설정 파일 (`~/.zshrc` 등)에 추가:

```bash
export XCODE_TOOLS_SIMULATOR="iPhone 15 Pro"
```

### 빌드 Configuration 변경

Task에서 이미 Debug/Release를 선택할 수 있지만, 기본값을 변경하려면:

```bash
export XCODE_TOOLS_CONFIG="Release"
```

---

## 문제 해결

### "No .xcworkspace or .xcodeproj found" 에러

Zed에서 Xcode 프로젝트 **루트 폴더**를 열었는지 확인하세요. `.xcodeproj` 또는 `.xcworkspace` 파일이 있는 폴더여야 합니다.

### "No schemes found" 에러

Xcode에서 해당 프로젝트를 한 번 열어서 scheme을 생성해야 합니다. 또는 `.xcscheme` 파일이 `xcshareddata/xcschemes/`에 있는지 확인하세요.

### Task가 보이지 않음

setup 스크립트를 실행했는지 확인하세요:
```bash
cd Alternative-Xcode
bash scripts/setup.sh
```

### xcbeautify 설치 방법

```bash
brew install xcbeautify
```

설치하지 않아도 동작합니다. 설치하면 빌드 출력이 깔끔해질 뿐입니다.

---

## 프로젝트 구조

```
Alternative-Xcode/
├── extension.toml          # Zed extension 매니페스트
├── Cargo.toml              # Rust 의존성
├── src/
│   └── lib.rs              # Extension 코어 (DAP 디버깅)
├── scripts/
│   ├── helpers.sh          # 빌드/실행/테스트/클린 셸 함수
│   └── setup.sh            # 1회 설정 스크립트
├── debug_adapter_schemas/
│   └── xcode-debug.json    # 디버그 설정 스키마
├── languages/
│   └── swift/
│       └── config.toml     # 언어 선언
├── PRD.md                  # 제품 요구사항
└── docs/
    ├── v0.1_spec.md        # v0.1 상세 기획
    └── architecture.md     # 아키텍처 및 설계
```

## 라이선스

[Apache License 2.0](./LICENSE)
