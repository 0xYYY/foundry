{% for contract in contracts %}
# {{ contract.name }}.sol

## User Doc
### Methods
{% for (func, doc) in contract.userdoc.methods %}
```
{{ func }}
```
{% endfor %}

### Events
### Errors

## Dev Doc
### Methods
### Events
### Errors
{% endfor %}
