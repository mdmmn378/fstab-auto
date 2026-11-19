# A throwaway Samba server standing in for a NAS in the end-to-end test.
FROM alpine:3.22
RUN apk add --no-cache samba
COPY smb.conf /etc/samba/smb.conf
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
EXPOSE 445
ENTRYPOINT ["/entrypoint.sh"]
