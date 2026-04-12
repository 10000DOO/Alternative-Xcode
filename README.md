# Xcode Tools for Zed

Zed 에디터에서 Xcode 없이 iOS/macOS 앱을 **빌드, 실행, 테스트, 디버그**할 수 있게 해주는 extension.

기존 Swift/Objective-C extension과 충돌 없이 공존하며, Xcode 워크플로우 대체에만 집중합니다.

## 주요 기능

| 기능 | 설명 |
|------|------|
| **Build** | `xcodebuild build` — scheme 자동 탐색 + 선택 |
| **Run** | macOS 앱 실행 / iOS Simulator 설치 + 실행 |
| **Test** | `xcodebuild test` — XCTest, Swift Testing 지원 |
| **Clean** | `xcodebuild clean` |
| **Debug** | DAP 기반 브레이크포인트 디버깅 (lldb-dap) |

## 요구 사항

- macOS
- [Zed](https://zed.dev)
- Xcode + Command Line Tools
- (권장) [Swift extension](https://zed.dev/extensions) 설치

## 상태

현재 **기획/설계 단계**입니다. 자세한 내용은 아래 문서를 참조하세요.

| 문서 | 내용 |
|------|------|
| [PRD.md](./PRD.md) | 제품 요구사항 |
| [docs/v0.1_spec.md](./docs/v0.1_spec.md) | v0.1 상세 기획 |
| [docs/architecture.md](./docs/architecture.md) | 아키텍처 및 설계 다이어그램 |

## 라이선스

[MIT](./LICENSE)
