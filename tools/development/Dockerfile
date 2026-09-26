FROM ghcr.io/astral-sh/uv:0.10.2@sha256:94a23af2d50e97b87b522d3cea24aaf8a1faedec1344c952767434f69585cbf9 AS uv
FROM ubuntu:26.04@sha256:da6fc2be547864451aa253836dd926da33623312df4a9a243e35dc877c378a78 AS runtime

COPY --from=uv /uv /usr/local/bin/uv
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
       darktable=5.4.1-1 ca-certificates libgomp1 \
    && dpkg-query -W > /opt/os-packages.txt \
    && rm -rf /var/lib/apt/lists/*
ENV UV_PYTHON_INSTALL_DIR=/opt/python UV_PYTHON_BIN_DIR=/usr/local/bin UV_LINK_MODE=copy
RUN uv python install 3.13.12 && uv venv --python 3.13.12 /opt/runtime
COPY requirements.lock /opt/requirements.lock
RUN uv pip sync --python /opt/runtime/bin/python --require-hashes /opt/requirements.lock

# Keep the original source and license notices together in the qualification image.
ADD --checksum=sha256:b2aca043227bb76f7f09eef4466cd8f9410f0cd5832ae2f4d045b80cb13b328f \
    https://codeload.github.com/andreavolpato/spektrafilm/tar.gz/3bb2c2d2801ff68b92019cf1dbcbb133d60832bc /opt/spektrafilm.tar.gz
RUN mkdir /opt/spektrafilm \
    && tar -xzf /opt/spektrafilm.tar.gz --strip-components=1 -C /opt/spektrafilm \
    && rm /opt/spektrafilm.tar.gz
COPY patches/0001-bounded-output-gamut.patch \
     patches/0002-buffer-lifetimes-and-output.patch /opt/patches/
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends patch \
    && patch --batch --fuzz=0 -p1 -d /opt/spektrafilm < /opt/patches/0001-bounded-output-gamut.patch \
    && patch --batch --fuzz=0 -p1 -d /opt/spektrafilm < /opt/patches/0002-buffer-lifetimes-and-output.patch \
    && dpkg-query -W > /opt/os-packages.txt \
    && rm -rf /var/lib/apt/lists/*
COPY bounded_gamut.py /opt/spektrafilm/src/spektrafilm/utils/bounded_gamut.py
COPY bounded_output.py /opt/spektrafilm/src/spektrafilm/utils/bounded_output.py
COPY film.py film_identity.py probe.py bundle.py history.py /opt/probe/
COPY engine_checks/test_bounded_gamut.py engine_checks/test_buffer_lifetimes.py \
     engine_checks/run.py engine_checks/output_scratch_probe.py \
     engine_checks/cold_lifetime_probe.py \
     engine_checks/pipeline_reference_probe.py \
     engine_checks/scratch_probe.py engine_checks/requirements.lock \
     engine_checks/0001-current-grain-contract.patch /opt/engine_checks/
COPY Dockerfile /opt/development-Dockerfile
RUN /opt/runtime/bin/python /opt/probe/bundle.py
ENV PATH=/opt/runtime/bin:$PATH PYTHONPATH=/opt/spektrafilm/src \
    PYTHONDONTWRITEBYTECODE=1 MPLBACKEND=Agg MPLCONFIGDIR=/work/matplotlib \
    XDG_CACHE_HOME=/work/xdg XDG_CONFIG_HOME=/work/xdgconfig NUMBA_CACHE_DIR=/work/numba \
    OMP_NUM_THREADS=4 OPENBLAS_NUM_THREADS=4 NUMBA_NUM_THREADS=1 NUMEXPR_NUM_THREADS=4
WORKDIR /work
ENTRYPOINT ["python", "/opt/probe/probe.py"]

# Test tools do not enter the qualification runtime or its dependency identity.
FROM runtime AS engine-checks
RUN patch --batch --fuzz=0 -p1 -d /opt/spektrafilm \
    < /opt/engine_checks/0001-current-grain-contract.patch
RUN uv pip install --python /opt/runtime/bin/python --target /opt/checks-deps \
    --require-hashes -r /opt/engine_checks/requirements.lock
ENV PYTHONPATH=/opt/checks-deps:/opt/spektrafilm/src
ENTRYPOINT ["python", "/opt/engine_checks/run.py"]

FROM runtime AS qualification
