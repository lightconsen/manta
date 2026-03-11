# Kubernetes Deployment

This directory contains Kubernetes manifests for deploying Manta.

## Quick Start

### Prerequisites

- Kubernetes cluster (1.24+)
- kubectl configured
- Kustomize (optional but recommended)

### Deploy with Kustomize

```bash
cd deploy/kubernetes

# Create the namespace and all resources
kubectl apply -k .
```

### Deploy without Kustomize

```bash
kubectl apply -f namespace.yaml
kubectl apply -f configmap.yaml
kubectl apply -f secret.yaml
kubectl apply -f deployment.yaml
kubectl apply -f service.yaml
kubectl apply -f hpa.yaml
```

## Configuration

### Set API Key

```bash
kubectl create secret generic manta-secrets \
  --from-literal=MANTA_API_KEY=your-api-key \
  --namespace=manta
```

### Update Config

Edit `configmap.yaml` and apply:

```bash
kubectl apply -f configmap.yaml
kubectl rollout restart deployment/manta -n manta
```

## Scaling

### Horizontal Pod Autoscaler

The HPA is configured to scale based on CPU and memory usage:

```bash
# View HPA status
kubectl get hpa -n manta

# Manually scale
kubectl scale deployment/manta --replicas=3 -n manta
```

## Monitoring

### Check Pod Status

```bash
kubectl get pods -n manta
kubectl logs -f deployment/manta -n manta
```

### Exec into Pod

```bash
kubectl exec -it deployment/manta -n manta -- /bin/sh
```

## Ingress

Update `service.yaml` with your domain:

```yaml
spec:
  rules:
    - host: manta.yourdomain.com
```

Apply:

```bash
kubectl apply -f service.yaml
```

## Troubleshooting

### Pod won't start

```bash
kubectl describe pod -n manta -l app.kubernetes.io/name=manta
kubectl logs -n manta -l app.kubernetes.io/name=manta --previous
```

### PVC issues

```bash
kubectl get pvc -n manta
kubectl describe pvc manta-data -n manta
```

### Resource limits

Check if pods are being OOMKilled:

```bash
kubectl get events -n manta --field-selector reason=OOMKilled
```

## Security

- Runs as non-root user (UID 1000)
- Read-only root filesystem
- Dropped all capabilities
- Resource limits enforced
