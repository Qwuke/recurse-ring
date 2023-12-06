FROM rust:latest AS builder

RUN apt update && apt install -y musl-tools musl-dev pkg-config libssl-dev ca-certificates
RUN apt-get install -y build-essential
RUN yes | apt install gcc-x86-64-linux-gnu
RUN rustup default nightly
ENV PATH $HOME/.cargo/bin:$PATH

WORKDIR /app

COPY ./ .
ENV RUSTFLAGS='-C linker=x86_64-linux-gnu-gcc'
RUN rustup default nightly
RUN cargo build --release

FROM ubuntu:latest
ARG DEBIAN_FRONTEND=noninteractive

RUN apt update
RUN apt install -y libpq-dev ca-certificates
ENV ROCKET_ADDRESS=0.0.0.0
ENV ROCKET_PORT=4000
EXPOSE 4000
COPY --from=builder /app/target/release/recurse-ring /usr/local/bin/recurse-ring
COPY --from=builder /app/templates/ /app/templates/
COPY --from=builder /app/templates/ /usr/local/bin/app/templates/
COPY --from=builder /app/templates/ /usr/local/bin/templates/
COPY --from=builder /app/static/ /app/static/
COPY --from=builder /app/static/ /usr/local/bin/app/static/

WORKDIR /usr/local/bin
CMD ["recurse-ring"]