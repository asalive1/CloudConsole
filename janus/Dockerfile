FROM ladamalina/janus-gateway:v1.4.3
COPY janus/entrypoint.sh /usr/local/bin/cloud-console-janus
RUN chmod 755 /usr/local/bin/cloud-console-janus
ENTRYPOINT ["/usr/local/bin/cloud-console-janus"]
