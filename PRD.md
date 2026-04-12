# PRD: Xcode Tools for Zed

## 1. 개요

Zed 에디터에서 Xcode 없이 iOS/macOS 앱을 빌드, 실행, 테스트, 디버그할 수 있게 해주는 **Xcode 도구 통합 extension**.

기존 Swift/Objective-C extension과 **충돌 없이 공존**하며, 언어 기능(문법, LSP)은 제공하지 않고 **Xcode 워크플로우 대체**에만 집중한다.

### 관련 프로젝트

| | Xcode Tools Extension | Xcode MCP (별도) |
|---|---|---|
| **사용 주체** | 개발자 본인 | Zed AI Agent |
| **인터페이스** | Task + DAP | MCP 프로토콜 |
| **결과 형태** | 터미널 출력 | 구조화된 JSON |

장기적으로 하나의 extension으로 통합하여 Task/DAP(사람용) + MCP(AI용) 모두 지원.

---

## 2. 배경 및 문제

| 문제 | 설명 |
|------|------|
| 빌드 불가 | Zed에서 xcodebuild 기반 빌드를 트리거할 방법 없음 |
| 실행 불가 | iOS Simulator / macOS 앱 실행 경로 없음 |
| 디버그 제한 | Xcode 프로젝트의 브레이크포인트 기반 디버깅 불가 |
| 테스트 불편 | xcodebuild test를 터미널에서 수동 실행해야 함 |

**접근 방식**: xcodebuild / xcrun simctl 등 Apple 기본 CLI를 직접 호출. 외부 의존성 최소화.

---

## 3. 목표

### 달성 목표
1. Zed에서 **빌드/실행/테스트/디버그**를 extension 내에서 수행
2. 기존 Swift/Objective-C extension과 **충돌 없이** 공존
3. Xcode + CLI Tools 외 **추가 설치 없이** 사용
4. Zed에서 **브레이크포인트 기반 디버깅** 가능
5. Swift 개발자가 Zed를 **메인 개발 환경 후보**로 고려할 수 있는 수준

### 비달성 목표
- Tree-sitter 문법, SourceKit-LSP 관리 (기존 extension 영역)
- SwiftUI Preview, Interface Builder, .pbxproj 편집
- 인증서/프로비저닝 관리
- SPM 전용 프로젝트 (향후 지원 예정)

---

## 4. 사용자

**타겟**: Zed를 사용 중이거나 관심 있는 iOS/macOS Swift 개발자. CLI 워크플로우에 익숙한 개발자.

**전제 조건**: macOS + Xcode + Command Line Tools 설치. 기존 Swift extension 설치 권장.

---

## 5. 핵심 기능

### 5.1 빌드 Task
- `xcodebuild build -scheme <scheme> -destination <dest>` 직접 호출
- `.xcodeproj` / `.xcworkspace` 자동 감지
- Scheme 탐색 (shared + user scheme 모두) → 번호 선택
- `xcbeautify` 설치 시 자동 파이프, 미설치 시 raw 출력
- 지원 타겟: macOS, iOS Simulator, iOS Device

### 5.2 실행 Task
- macOS: 빌드 후 `open .app`
- iOS Simulator: 빌드 후 `simctl boot` → `install` → `launch`
- `xcodebuild -showBuildSettings` 1회 호출로 빌드 산출물 경로 캐싱

### 5.3 테스트 Task
- `xcodebuild test` 호출
- 전체 / 특정 타겟 단위 실행
- `runnables.scm`을 통한 인라인 테스트 실행 버튼

### 5.4 클린 Task
- `xcodebuild clean` 호출

### 5.5 디버깅 (DAP)
- Zed DAP 시스템으로 **브레이크포인트 기반 디버깅**
- v0.1: 기존 lldb-dap 활용 (macOS 앱)
- v0.2+: 자체 DAP 래퍼로 iOS Simulator attach
- v1.0: iOS Device attach

### 5.6 swift-format 연동 (v0.2)
- 바이너리 자동 감지, 저장 시 자동 포맷

---

## 6. 사용자 설정

```jsonc
// .zed/settings.json (프로젝트별) 또는 ~/.config/zed/settings.json (글로벌)
{
  "xcode-tools": {
    "simulator": "iPhone 16",
    "use_xcbeautify": true
  }
}
```

| 설정 | 입력 시점 | v0.1 구현 | v0.2 확장 |
|------|-----------|-----------|-----------|
| Scheme | 매번 선택 | interactive read | 동일 |
| Simulator | 1회 설정 | 환경변수 기본값 | settings.json |
| Configuration | 1회 설정 | 환경변수 기본값 | settings.json |

---

## 7. 제약 조건

| 제약 | 영향 | 대응 |
|------|------|------|
| WASM 샌드박스 | 직접 프로세스 실행 불가 | Task로 빌드/실행, DAP API로 디버그 |
| 커스텀 UI 불가 | 빌드 로그 뷰어 불가 | 터미널 출력 의존 |
| runnables.scm은 grammar 소유 extension 필요 | 검증 필요 | 불가 시 Task로 대체 |
| Gallery 이름 제약 | "zed", "extension" 포함 불가 | "Xcode Tools" 사용 |
| macOS 전용 | - | Apple 도구 특성 |
| .xcodeproj/.xcworkspace만 지원 | SPM 전용 미지원 | 향후 지원 예정 |

---

## 8. 로드맵

> 버전별 상세 기획: [docs/v0.1_spec.md](./docs/v0.1_spec.md)
> 아키텍처/설계: [docs/architecture.md](./docs/architecture.md)

| 차수 | 버전 | 핵심 결과물 |
|------|------|-------------|
| **1차** | v0.1 | Task(빌드/실행/테스트/클린) + 기본 DAP(macOS 디버깅) |
| **2차** | v0.2~v0.3 | 자체 DAP 래퍼(Simulator 디버깅) + swift-format + runnables.scm + UX |
| **3차** | v1.0 | iOS Device 디버깅 + Extension Gallery 등록 |
| **4차** | v2.0 | Xcode MCP 통합 (AI 자동화) |

---

## 9. 성공 지표

| 지표 | 기준 |
|------|------|
| 충돌 없음 | Swift/ObjC extension 동시 설치 시 문제 없음 |
| 빌드 | xcodebuild로 프로젝트 빌드 가능 |
| 실행 | macOS 앱 / Simulator 앱 실행 가능 |
| 디버깅 | 브레이크포인트, 변수 조회, 스텝 실행 동작 |
| 테스트 | xcodebuild test 결과 확인 가능 |
| 설치 용이성 | Extension 설치만으로 사용 가능 (Xcode만 필요) |

---

## 10. 미결 사항

| 항목 | 상태 | 비고 |
|------|------|------|
| Extension 라이선스 | **Apache 2.0** | LICENSE 파일 확정 |
| `languages/swift/tasks.json` 동작 여부 | S1-2에서 검증 | 미동작 시 `.zed/tasks.json` 전환 |
| runnables.scm 제공 가능 여부 | v0.2에서 검증 | grammar 미소유 시 불가 |
| MCP 서버 런타임 | v2.0에서 결정 | Node.js vs Rust |

---

## 참고 자료

- [Zed Debugger Extensions](https://zed.dev/docs/extensions/debugger-extensions) / [Task 시스템](https://zed.dev/docs/tasks)
- [Zed Extension 개발](https://zed.dev/docs/extensions/developing-extensions) / [Debugger 블로그](https://zed.dev/blog/debugger)
- [zed_extension_api](https://docs.rs/zed_extension_api) / [zed-extensions/swift](https://github.com/zed-extensions/swift)
- [xcbeautify](https://github.com/cpisciotta/xcbeautify) / [zed-industries/extensions](https://github.com/zed-industries/extensions)
