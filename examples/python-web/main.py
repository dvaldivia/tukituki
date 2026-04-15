import logging
import os
from http.server import HTTPServer, BaseHTTPRequestHandler

from opentelemetry.exporter.otlp.proto.grpc._log_exporter import OTLPLogExporter
from opentelemetry.sdk._logs import LoggerProvider, LoggingHandler
from opentelemetry.sdk._logs.export import BatchLogRecordProcessor
from opentelemetry.sdk.resources import Resource

# Set up OTel log export.
resource = Resource.create({"service.name": "python-web"})
provider = LoggerProvider(resource=resource)

endpoint = os.getenv("OTEL_EXPORTER_OTLP_ENDPOINT", "http://localhost:4317")
# The gRPC exporter wants host:port without scheme.
grpc_endpoint = endpoint.replace("http://", "").replace("https://", "")

provider.add_log_record_processor(
    BatchLogRecordProcessor(OTLPLogExporter(endpoint=grpc_endpoint, insecure=True))
)

handler = LoggingHandler(level=logging.NOTSET, logger_provider=provider)
logging.getLogger().addHandler(handler)
logging.getLogger().setLevel(logging.DEBUG)

logger = logging.getLogger("python-web")


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        for i in range(10):
            logger.info("handling request step=%d path=%s", i, self.path)
        logger.error("upstream service unavailable: connection refused to auth-svc:443")
        for i in range(10, 20):
            logger.info("request processing step=%d", i)
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"ok\n")

    def log_message(self, format, *args):
        # Silence default stderr access logs.
        pass


if __name__ == "__main__":
    port = 8083
    server = HTTPServer(("", port), Handler)
    print(f"python-web listening on :{port}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("python-web shutting down")
        provider.shutdown()
