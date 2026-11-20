FROM rust:1.96.0-trixie@sha256:58fe97504a0e4cbba5d85599619a589923d3e779472a6fb0840d58d1c4ba99d7 AS rust-base

FROM rust-base AS ffmpeg-build
RUN apt-get update && apt-get install -y --no-install-recommends \
    libx264-dev=2:0.164.3108+git31e19f9-2+b1 nasm=2.16.03-1
ADD --checksum=sha256:8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e \
    https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz /tmp/ffmpeg.tar.xz
WORKDIR /ffmpeg
RUN tar -xJf /tmp/ffmpeg.tar.xz --strip-components=1 && \
    ./configure --prefix=/opt/ffmpeg --disable-autodetect --disable-doc --disable-debug \
      --disable-ffplay --disable-network --disable-protocols --enable-protocol=file,pipe \
      --disable-devices --enable-indev=lavfi --enable-gpl --enable-libx264 --enable-zlib && \
    make -j"$(nproc)" install-progs
RUN mkdir -p /runtime/var/lib/dpkg/status.d && \
    for spec in "libx264-164:/libx264\.so\.164$" "zlib1g:/libz\.so\.1$"; do \
      pkg="${spec%%:*}"; pattern="${spec#*:}"; \
      library="$(dpkg-query -L "$pkg" | grep -E "$pattern" | head -n 1)" && \
      test -f "$library" && install -D "$library" "/runtime$library" && \
      dpkg-query -s "$pkg" > "/runtime/var/lib/dpkg/status.d/$pkg"; \
    done

FROM rust-base AS api-build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --locked --release --bin api
RUN install -d -m 0700 /data

FROM api-build AS test-build
RUN cargo test --locked --no-run && mkdir /test-binaries && \
    find target/debug/deps -type f -perm /111 \( -name 'vidora-*' -o -name 'api-*' \) \
      -exec cp {} /test-binaries/ \;

FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2 AS test
COPY --from=ffmpeg-build /runtime/ /
COPY --from=ffmpeg-build /opt/ffmpeg/bin/ /usr/local/bin/
COPY --from=test-build /test-binaries/ /test-binaries/
USER 10001:10001
RUN test "$(ls -1 /test-binaries | wc -l)" -eq 3 && \
    for suite in /test-binaries/*; do "$suite" || exit 1; done

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cb87acc574abe719b6e5c547384defa2f7b1f24af9605d3f618cf852495d8be2 AS production
COPY --from=ffmpeg-build /runtime/ /
COPY --from=ffmpeg-build /opt/ffmpeg/bin/ /usr/local/bin/
COPY --from=api-build /build/target/release/api /usr/local/bin/vidora-api
COPY --chown=10001:10001 --from=api-build /data /var/lib/vidora
USER 10001:10001
ENV DATA_DIR=/var/lib/vidora
ENTRYPOINT ["vidora-api"]
