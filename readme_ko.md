# DirectShare
여러 파일들을 간편하게 셀프 호스팅 할 수 있게 해줍니다.

## 기능
* 자동 uPnP 포트포워딩
* 포트, 단축 URL 길이 설정 가능
* 폴더 tar 파일로 스트리밍

## 사용법
공유 할 파일들을 프로그램에 드래그하여 실행한뒤 생성된 단축 url로 접속하면 다운로드 할 수 있습니다.
폴더의 경우 tar 파일로 공유됩니다.

```
registered foo.txt url: http://127.0.0.1:1024/xIqfLguw
```
`http://127.0.0.1:1024/xIqfLguw` 가 foo.txt 파일을 받을수 있는 주소 입니다.
`direct_share.toml` 파일에서 단축 url의 주소 길이와 포트 번호를 설정 할 수 있습니다.

## License
`DirectShare` is following MIT License