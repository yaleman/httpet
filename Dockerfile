
FROM rust:1-slim-trixie AS builder


# fixing the issue with getting OOMKilled in BuildKit
RUN mkdir /httpet
COPY . /httpet/

WORKDIR /httpet
# install the dependencies
RUN apt-get update && apt-get -q install -y \
    git \
    clang \
    pkg-config \
    mold
ENV CC="/usr/bin/clang"
RUN cargo build --quiet --release --bin httpet
RUN chmod +x /httpet/target/release/httpet

FROM gcr.io/distroless/cc-debian12 AS final
# FROM rust:1.90.0-slim-trixie AS secondary


# RUN apt-get -y remove --allow-remove-essential \
#     binutils cpp cpp-14 gcc gcc grep gzip ncurses-bin ncurses-base sed && apt-get autoremove -y && apt-get clean && rm -rf /var/lib/apt/lists/* && rm -rf /usr/local/cargo /usr/local/rustup


# # # ======================
# # https://github.com/GoogleContainerTools/distroless/blob/main/examples/rust/Dockerfile
# COPY --from=builder /httpet/target/release/httpet /
# COPY --from=builder /httpet/static /static
# COPY --from=js-builder /app/static/js/* /static/js/
# WORKDIR /
# RUN useradd -m nonroot

# FROM scratch AS final
ARG GITHUB_SHA="$(git rev-parse HEAD)"
LABEL com.httpet.git-commit="${GITHUB_SHA}"

ARG DESCRIPTION="$(./scripts/get_description.sh)"
LABEL description="${DESCRIPTION}"

WORKDIR /

COPY --from=builder /httpet/target/release/httpet /httpet
ADD ./static /static

USER nonroot
ENTRYPOINT ["./httpet"]

CMD ["--listen-address","0.0.0.0"]

