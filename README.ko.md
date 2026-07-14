# Wuwa ini Tool

Wuwa ini Tool은 Wuthering Waves(명조)의 `Engine.ini` 설정과 게임 실행 중의
임시 CPU 설정을 관리하는 비공식 오픈 소스 Windows 데스크톱 도구입니다.
Tauri v2, Rust, React, TypeScript로 제작됩니다.

영문 안내는 [README.md](README.md)를 참고하세요.

> [!WARNING]
> 이 프로젝트는 Kuro Games와 제휴하거나 Kuro Games의 승인·지원을 받은
> 프로그램이 아닙니다. 설정과 프로세스 변경으로 게임 크래시, 설정 손실,
> 성능 저하 또는 계정 제재가 발생할 수 있습니다. 성능 향상이나 계정 안전을
> 보장하지 않습니다. 사용 전에 [한영 면책 고지](DISCLAIMER.md)를 확인하세요.

## 안전 원칙

- 선택한 게임 설치 경로에서 검증된
  `Client/Saved/Config/WindowsNoEditor/Engine.ini`만 편집합니다.
  `UserEngine.ini` 같은 우회 파일을 만들거나 사용하지 않습니다.
- 파일을 쓰기 전에 diff를 보여 주고 명시적인 확인을 받습니다.
- 적용 및 복원 전에 원본 바이트와 동일한 백업을 만들고 해시로 검증합니다.
- 관련 없는 섹션, 주석, 순서, 줄바꿈, 지원 인코딩, 알 수 없는 키와 중복 키
  증거를 보존합니다.
- 가져온 프로필과 붙여넣은 INI 전체 문서를 신뢰하지 않는 입력으로 검사합니다.
- 캐시 삭제는 허용 목록 경로 안의 내용만 대상으로 하며 재분석 지점을 따라가지
  않습니다.
- 코드 주입, 게임 메모리 접근, 안티치트 후킹, 드라이버 설치, IFEO 변경 또는
  게임의 기술적 제한 우회를 하지 않습니다.

## 주요 기능

- 한국어/영어 UI, 시스템/라이트/다크 테마
- 보수적인 기본, 균형, 성능, 화질 프리셋
- 옵션별 한영 설명, 근거 상태, 위험 경고, 출처 링크
- 전체 `Engine.ini` 붙여넣기/가져오기와 사용자 지정 섹션·키·값
- 통합/분할 diff, 최초 원본 영구 보존, 고정 백업, 무결성 검증 복원
- 로컬 경로·장치 식별자·백업 기록이 빠진 이식 가능한 프로필 공유
- Windows의 모든 우선도 등급과 토폴로지 기반 CPU Sets/고급 affinity
- 통신·녹화·오디오·포그라운드·시스템 프로세스를 보호하는 선택형 Focus Mode
- 명조 캐시와 현재 사용자 NVIDIA 셰이더 캐시의 개별 미리보기 및 삭제
- 서명된 GitHub 업데이트 확인과 사용자 승인 후 설치

옵션이 `Engine.ini`에 남아 있는 것만으로 게임이 그 옵션을 실제 사용했다고 볼 수
없습니다. 커뮤니티 제보 및 실험 옵션은 명확히 구분하며 재현 가능한 근거 없이
자동 프리셋으로 승격하지 않습니다.

## 현재 상태

`1.0.0`은 첫 공개 배포 목표 버전입니다. 검증된 GitHub Release가 게시되기
전까지는 소스에서 빌드하거나 초안 산출물을 테스트 용도로만 사용하세요. 보호된
업데이터 서명 비밀 또는 저장소의 공개 키가 없으면 릴리스 자동화는 의도적으로
실패합니다.

## 소스 빌드

Windows 10/11 x64, Node.js 22 이상, npm, MSVC용 Rust stable, Visual Studio
Build Tools의 C++ 데스크톱 워크로드와 WebView2가 필요합니다.

```powershell
git clone https://github.com/INIRU/Wuwa-ini-Tool.git
cd Wuwa-ini-Tool
npm ci
npm run tauri dev
```

전체 검사 명령은 [영문 README의 Build from source](README.md#build-from-source)를
참고하세요. NSIS 설치 파일은 `npm run tauri build -- --bundles nsis`로 만들 수
있지만, 공개 자동 업데이트 산출물은 별도의 보호된 서명 키가 필요합니다.

## 참여 및 지원

- PR 전 [CONTRIBUTING.md](CONTRIBUTING.md)를 확인하세요.
- 버그, 기능 요청, 옵션 근거는
  [구조화된 Issue 양식](https://github.com/INIRU/Wuwa-ini-Tool/issues/new/choose)을
  사용하세요.
- 일반 지원 범위는 [SUPPORT.md](SUPPORT.md), 보안 취약점 비공개 신고 방법은
  [SECURITY.md](SECURITY.md)를 확인하세요.
- 라이선스가 불명확하거나 GPL 등 MIT 프로젝트와 호환되지 않는 코드·설정·문구·
  이미지를 복사하지 마세요. 공개 게시물은 참고 근거이지 복제 허가가 아닙니다.

## 라이선스와 고지

소스 코드는 [MIT License](LICENSE)로 공개됩니다. MIT 라이선스와 프로젝트의
[면책 고지](DISCLAIMER.md)는 별도 문서이며, 게임 이용약관이나 제3자의 권리를
변경하지 않습니다.
